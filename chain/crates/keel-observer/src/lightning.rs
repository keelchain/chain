//! Lightning worker (2026-09-10): the observer's LND node as a hot
//! pool next to the vault. Three passes and one tiny HTTP endpoint:
//!
//! - `register_once`: the node's identity pubkey is registered on chain
//!   (`RegisterLightningNode`) so invoices it signs are accepted.
//! - `deposits_once`: settled invoices whose memo binds an KEEL account
//!   (`keel:<address>`) become `ObserveLightningDeposit` with the preimage.
//! - `payouts_once`: payouts the chain assigned to this observer are paid
//!   (fee-capped by the chain's allowance) and reported with the preimage.
//! - `POST /v1/lightning/invoice {owner, amount_sat}`: the marketplace asks
//!   for a deposit invoice; the memo carries the owner, nothing else is
//!   trusted from the caller.

use crate::config::LightningConfig;
use crate::rpc::{SttRpc, Submitter};
use crate::state::StateFile;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use base64::Engine as _;
use keel_actions::{Action, LightningDepositObservation};
use keel_types::Address;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// What the worker needs from LND (REST). Mocked in tests.
#[async_trait]
pub trait LndApi: Send + Sync {
    /// Identity pubkey, 33 bytes.
    async fn node_id(&self) -> anyhow::Result<Vec<u8>>;
    async fn add_invoice(
        &self,
        memo: &str,
        amount_msat: u64,
        expiry_secs: u64,
    ) -> anyhow::Result<CreatedInvoice>;
    /// Invoices settled after `settle_index`, oldest first, with the new index.
    async fn settled_since(&self, settle_index: u64) -> anyhow::Result<(Vec<SettledInvoice>, u64)>;
    /// Pays `bolt11` with at most `fee_limit_sat` in routing fees.
    async fn pay(&self, bolt11: &str, fee_limit_sat: u64) -> anyhow::Result<PayResult>;
    async fn new_address(&self) -> anyhow::Result<String>;
    async fn channel_balance_sat(&self) -> anyhow::Result<u64>;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatedInvoice {
    pub payment_request: String,
    pub payment_hash: [u8; 32],
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettledInvoice {
    pub payment_request: String,
    pub memo: String,
    pub preimage: [u8; 32],
    pub amount_paid_msat: u64,
    pub settle_index: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayResult {
    pub success: bool,
    pub preimage: Option<[u8; 32]>,
    pub fee_paid_msat: u64,
    pub error: Option<String>,
}

// ---------------------------------------------------------------- LND REST

pub struct HttpLnd {
    client: reqwest::Client,
    base: String,
    macaroon_hex: String,
}

impl HttpLnd {
    pub fn new(cfg: &LightningConfig) -> anyhow::Result<Self> {
        let macaroon_hex = match (&cfg.macaroon_hex, &cfg.macaroon_path) {
            (Some(h), _) => h.trim().to_string(),
            (None, Some(p)) => hex::encode(
                std::fs::read(p).with_context(|| format!("read macaroon {}", p.display()))?,
            ),
            (None, None) => anyhow::bail!("lightning: set macaroon_hex or macaroon_path"),
        };
        let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(90));
        if cfg.tls_insecure {
            b = b.danger_accept_invalid_certs(true);
        }
        if let Some(cert) = &cfg.tls_cert_path {
            let pem =
                std::fs::read(cert).with_context(|| format!("read tls cert {}", cert.display()))?;
            b = b.add_root_certificate(reqwest::Certificate::from_pem(&pem)?);
        }
        Ok(Self {
            client: b.build()?,
            base: cfg.rest_url.trim_end_matches('/').to_string(),
            macaroon_hex,
        })
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        let mut req = self
            .client
            .request(method, format!("{}{}", self.base, path))
            .header("Grpc-Metadata-macaroon", &self.macaroon_hex);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.with_context(|| format!("lnd {path}"))?;
        let status = res.status();
        let text = res.text().await?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
        if !status.is_success() {
            anyhow::bail!(
                "lnd {path}: HTTP {status}: {}",
                v.get("message").and_then(Value::as_str).unwrap_or(&text)
            );
        }
        Ok(v)
    }
}

fn b64_32(v: &Value) -> Option<[u8; 32]> {
    let s = v.as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s)
        .ok()
        .or_else(|| base64::engine::general_purpose::URL_SAFE.decode(s).ok())?;
    <[u8; 32]>::try_from(bytes).ok()
}

fn num(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

#[async_trait]
impl LndApi for HttpLnd {
    async fn node_id(&self) -> anyhow::Result<Vec<u8>> {
        let v = self.call(reqwest::Method::GET, "/v1/getinfo", None).await?;
        let hexs = v
            .get("identity_pubkey")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("getinfo: no identity_pubkey"))?;
        Ok(hex::decode(hexs)?)
    }

    async fn add_invoice(
        &self,
        memo: &str,
        amount_msat: u64,
        expiry_secs: u64,
    ) -> anyhow::Result<CreatedInvoice> {
        let v = self.call(reqwest::Method::POST, "/v1/invoices", Some(json!({ "memo": memo, "value_msat": amount_msat.to_string(), "expiry": expiry_secs.to_string() }))).await?;
        let payment_request = v
            .get("payment_request")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("addinvoice: no payment_request"))?
            .to_string();
        let payment_hash = b64_32(v.get("r_hash").unwrap_or(&Value::Null))
            .ok_or_else(|| anyhow!("addinvoice: bad r_hash"))?;
        let expires_at = keel_ln::parse(&payment_request)
            .map(|i| i.expires_at)
            .unwrap_or(0);
        Ok(CreatedInvoice {
            payment_request,
            payment_hash,
            expires_at,
        })
    }

    async fn settled_since(&self, settle_index: u64) -> anyhow::Result<(Vec<SettledInvoice>, u64)> {
        // Invoices are listed by add index; filter to settled ones past our cursor.
        let v = self
            .call(
                reqwest::Method::GET,
                &format!(
                    "/v1/invoices?num_max_invoices=200&index_offset={settle_index}&reversed=false"
                ),
                None,
            )
            .await?;
        let mut out = Vec::new();
        let mut max_index = settle_index;
        for inv in v
            .get("invoices")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let add_index = num(inv.get("add_index").unwrap_or(&Value::Null));
            max_index = max_index.max(add_index);
            if inv.get("state").and_then(Value::as_str) != Some("SETTLED") {
                continue;
            }
            let Some(preimage) = b64_32(inv.get("r_preimage").unwrap_or(&Value::Null)) else {
                continue;
            };
            out.push(SettledInvoice {
                payment_request: inv
                    .get("payment_request")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                memo: inv
                    .get("memo")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                preimage,
                amount_paid_msat: num(inv.get("amt_paid_msat").unwrap_or(&Value::Null)),
                settle_index: num(inv.get("settle_index").unwrap_or(&Value::Null)),
            });
        }
        Ok((out, max_index))
    }

    async fn pay(&self, bolt11: &str, fee_limit_sat: u64) -> anyhow::Result<PayResult> {
        let v = self.call(reqwest::Method::POST, "/v1/channels/transactions", Some(json!({ "payment_request": bolt11, "fee_limit": { "fixed": fee_limit_sat.to_string() }, "allow_self_payment": true }))).await?;
        let error = v
            .get("payment_error")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let preimage = b64_32(v.get("payment_preimage").unwrap_or(&Value::Null));
        let fee_paid_msat = num(v
            .pointer("/payment_route/total_fees_msat")
            .unwrap_or(&Value::Null));
        Ok(PayResult {
            success: error.is_none() && preimage.is_some(),
            preimage,
            fee_paid_msat,
            error,
        })
    }

    async fn new_address(&self) -> anyhow::Result<String> {
        let v = self
            .call(
                reqwest::Method::POST,
                "/v1/newaddress",
                Some(json!({ "type": "WITNESS_PUBKEY_HASH" })),
            )
            .await?;
        v.get("address")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("newaddress: no address"))
    }

    async fn channel_balance_sat(&self) -> anyhow::Result<u64> {
        let v = self
            .call(reqwest::Method::GET, "/v1/balance/channels", None)
            .await?;
        Ok(num(v.pointer("/local_balance/sat").unwrap_or(&Value::Null)))
    }
}

// ---------------------------------------------------------------- worker

/// A chain-assigned payout, as `/v1/lightning` on the node reports it.
#[derive(Clone, Debug, Deserialize)]
pub struct AssignedPayout {
    pub outbound_id: u64,
    pub observer: String,
    pub deadline_height: u64,
    pub fee_allowance: String,
    pub invoice: Option<String>,
}

pub struct LightningWorker {
    pub lnd: Arc<dyn LndApi>,
    pub submitter: Arc<Submitter>,
    pub state: Arc<StateFile>,
    pub rpc: Arc<dyn SttRpc>,
    pub invoice_expiry_secs: u64,
}

impl LightningWorker {
    /// Registers the node id on chain once (and again if it changed).
    pub async fn register_once(&self) -> anyhow::Result<()> {
        let id = self.lnd.node_id().await?;
        let key = format!("ln:node:{}", hex::encode(&id));
        if self.state.is_submitted(&key) {
            return Ok(());
        }
        let r = self
            .submitter
            .submit(
                Some(&key),
                Action::RegisterLightningNode {
                    node_id: id.clone(),
                },
            )
            .await?;
        tracing::info!(
            node = hex::encode(&id),
            admitted = r.admitted,
            "lightning node registered"
        );
        Ok(())
    }

    /// Issues a deposit invoice bound to `owner`.
    pub async fn issue_invoice(
        &self,
        owner: Address,
        amount_sat: u64,
    ) -> anyhow::Result<CreatedInvoice> {
        if amount_sat == 0 {
            anyhow::bail!("amount must be > 0");
        }
        let memo = keel_ln::deposit_description(&owner.to_hex());
        self.lnd
            .add_invoice(&memo, amount_sat * 1_000, self.invoice_expiry_secs)
            .await
    }

    /// Reports settled KEEL invoices as deposits.
    pub async fn deposits_once(&self) -> anyhow::Result<usize> {
        let cursor = self.state.cursor("ln:invoice_index").unwrap_or(0);
        let (settled, next) = self.lnd.settled_since(cursor).await?;
        let mut n = 0;
        for inv in settled {
            if keel_ln::owner_of_description(&inv.memo).is_none() {
                continue;
            }
            let Ok(parsed) = keel_ln::parse(&inv.payment_request) else {
                continue;
            };
            let key = format!("ln:deposit:{}", hex::encode(parsed.payment_hash));
            if self.state.is_submitted(&key) {
                continue;
            }
            let r = self
                .submitter
                .submit(
                    Some(&key),
                    Action::ObserveLightningDeposit(LightningDepositObservation {
                        invoice: inv.payment_request.clone(),
                        preimage: inv.preimage,
                        amount_msat: inv.amount_paid_msat,
                    }),
                )
                .await?;
            tracing::info!(
                hash = hex::encode(parsed.payment_hash),
                msat = inv.amount_paid_msat,
                admitted = r.admitted,
                "lightning deposit reported"
            );
            n += 1;
        }
        if next > cursor {
            self.state.set_cursor("ln:invoice_index", next)?;
        }
        Ok(n)
    }

    /// Pays the payouts the chain assigned to this observer and reports.
    pub async fn payouts_once(&self, assignments: &[AssignedPayout]) -> anyhow::Result<usize> {
        let me = self.submitter.address().to_hex();
        let mut n = 0;
        for a in assignments
            .iter()
            .filter(|a| a.observer.eq_ignore_ascii_case(&me))
        {
            let key = format!("ln:payout:{}", a.outbound_id);
            if self.state.is_submitted(&key) {
                continue;
            }
            let Some(invoice) = a.invoice.as_deref() else {
                continue;
            };
            let fee_limit: u64 = a.fee_allowance.parse().unwrap_or(0);
            // Mark before paying: a crash between pay and report must not
            // pay twice. The operator reconciles a stuck one by hand.
            self.state
                .mark_submitted(&format!("ln:paying:{}", a.outbound_id), "")?;
            let result = match self.lnd.pay(invoice, fee_limit).await {
                Ok(r) => r,
                Err(e) => PayResult {
                    success: false,
                    preimage: None,
                    fee_paid_msat: 0,
                    error: Some(e.to_string()),
                },
            };
            let r = self
                .submitter
                .submit(
                    Some(&key),
                    Action::ObserveLightningPayout {
                        outbound_id: a.outbound_id,
                        preimage: result.preimage,
                        fee_paid_msat: result.fee_paid_msat,
                        success: result.success,
                    },
                )
                .await?;
            tracing::info!(outbound = a.outbound_id, success = result.success, fee_msat = result.fee_paid_msat, error = ?result.error, admitted = r.admitted, "lightning payout reported");
            n += 1;
        }
        Ok(n)
    }
}

/// `GET /v1/lightning` on the node → the assignments list.
pub fn parse_assignments(v: &Value) -> Vec<AssignedPayout> {
    v.get("assignments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| serde_json::from_value(x.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------- invoice API

/// Tiny HTTP surface for the marketplace: `POST /v1/lightning/invoice`
/// `{owner, amount_sat}` → `{payment_request, payment_hash, expires_at}`;
/// `GET /v1/lightning/info` → node id and channel balance.
pub async fn serve_api(listen: &str, worker: Arc<LightningWorker>) -> anyhow::Result<()> {
    use axum::{
        extract::State,
        routing::{get, post},
        Json, Router,
    };
    async fn invoice(
        State(w): State<Arc<LightningWorker>>,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
        let owner = body
            .get("owner")
            .and_then(Value::as_str)
            .and_then(Address::from_hex)
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "owner must be a 64-hex address" })),
                )
            })?;
        let amount_sat = num(body.get("amount_sat").unwrap_or(&Value::Null));
        match w.issue_invoice(owner, amount_sat).await {
            Ok(i) => Ok(Json(
                json!({ "payment_request": i.payment_request, "payment_hash": hex::encode(i.payment_hash), "expires_at": i.expires_at }),
            )),
            Err(e) => Err((
                axum::http::StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            )),
        }
    }
    async fn info(State(w): State<Arc<LightningWorker>>) -> Json<Value> {
        let node = w.lnd.node_id().await.map(hex::encode).unwrap_or_default();
        let balance = w.lnd.channel_balance_sat().await.unwrap_or(0);
        Json(
            json!({ "node_id": node, "observer": w.submitter.address().to_hex(), "channel_balance_sat": balance, "invoice_expiry_secs": w.invoice_expiry_secs }),
        )
    }
    let app = Router::new()
        .route("/v1/lightning/invoice", post(invoice))
        .route("/v1/lightning/info", get(info))
        .with_state(worker);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind {listen}"))?;
    tracing::info!(listen, "lightning invoice API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::rpc::MockSttRpc;
    use keel_actions::CHAIN_ID_DEVNET;
    use keel_crypto::Keypair;
    use std::sync::Mutex;

    struct MockLnd {
        settled: Mutex<Vec<SettledInvoice>>,
        paid: Mutex<Vec<(String, u64)>>,
        pay_ok: bool,
    }

    #[async_trait]
    impl LndApi for MockLnd {
        async fn node_id(&self) -> anyhow::Result<Vec<u8>> {
            Ok(vec![2u8; 33])
        }
        async fn add_invoice(
            &self,
            memo: &str,
            amount_msat: u64,
            _expiry: u64,
        ) -> anyhow::Result<CreatedInvoice> {
            Ok(CreatedInvoice {
                payment_request: format!("lnbcrt-mock-{memo}-{amount_msat}"),
                payment_hash: [1u8; 32],
                expires_at: 0,
            })
        }
        async fn settled_since(&self, idx: u64) -> anyhow::Result<(Vec<SettledInvoice>, u64)> {
            let all = self.settled.lock().unwrap().clone();
            let out: Vec<_> = all.into_iter().filter(|i| i.settle_index > idx).collect();
            let next = out.iter().map(|i| i.settle_index).max().unwrap_or(idx);
            Ok((out, next))
        }
        async fn pay(&self, bolt11: &str, fee_limit_sat: u64) -> anyhow::Result<PayResult> {
            self.paid
                .lock()
                .unwrap()
                .push((bolt11.to_string(), fee_limit_sat));
            Ok(if self.pay_ok {
                PayResult {
                    success: true,
                    preimage: Some([5u8; 32]),
                    fee_paid_msat: 2_500,
                    error: None,
                }
            } else {
                PayResult {
                    success: false,
                    preimage: None,
                    fee_paid_msat: 0,
                    error: Some("no route".into()),
                }
            })
        }
        async fn new_address(&self) -> anyhow::Result<String> {
            Ok("bcrt1qmock".into())
        }
        async fn channel_balance_sat(&self) -> anyhow::Result<u64> {
            Ok(1_000_000)
        }
    }

    fn worker(lnd: MockLnd) -> (LightningWorker, Arc<MockSttRpc>) {
        let rpc = Arc::new(MockSttRpc {
            chain_id: CHAIN_ID_DEVNET,
            ..Default::default()
        });
        let state = Arc::new(StateFile::ephemeral());
        let submitter = Arc::new(Submitter::new(
            rpc.clone(),
            Keypair::from_seed(3),
            CHAIN_ID_DEVNET,
            state.clone(),
        ));
        (
            LightningWorker {
                lnd: Arc::new(lnd),
                submitter,
                state,
                rpc: rpc.clone(),
                invoice_expiry_secs: 3_600,
            },
            rpc,
        )
    }

    fn kinds(rpc: &MockSttRpc) -> Vec<String> {
        rpc.submitted
            .lock()
            .unwrap()
            .iter()
            .map(|s| match &s.envelope.action {
                Action::RegisterLightningNode { .. } => "register".to_string(),
                Action::ObserveLightningDeposit(o) => format!("deposit:{}", o.amount_msat),
                Action::ObserveLightningPayout {
                    outbound_id,
                    success,
                    fee_paid_msat,
                    ..
                } => format!("payout:{outbound_id}:{success}:{fee_paid_msat}"),
                _ => "other".into(),
            })
            .collect()
    }

    #[tokio::test]
    async fn registers_once_and_reports_only_bound_settled_invoices_once() {
        // Real signed invoices are not needed here: the chain verifies them;
        // the worker only filters by memo and dedups by payment hash, and a
        // parse failure skips the invoice. Use a real one for the happy path.
        let owner = Address::from_hex(&"ab".repeat(32)).unwrap();
        let bolt11 =
            crate::lightning::tests::real_invoice(&keel_ln::deposit_description(&owner.to_hex()));
        let lnd = MockLnd {
            settled: Mutex::new(vec![
                SettledInvoice {
                    payment_request: bolt11.clone(),
                    memo: keel_ln::deposit_description(&owner.to_hex()),
                    preimage: [7u8; 32],
                    amount_paid_msat: 2_500_000,
                    settle_index: 1,
                },
                SettledInvoice {
                    payment_request: "lnbcrt1junk".into(),
                    memo: "coffee".into(),
                    preimage: [8u8; 32],
                    amount_paid_msat: 1,
                    settle_index: 2,
                },
            ]),
            paid: Mutex::new(vec![]),
            pay_ok: true,
        };
        let (w, rpc) = worker(lnd);
        w.register_once().await.unwrap();
        w.register_once().await.unwrap();
        assert_eq!(w.deposits_once().await.unwrap(), 1);
        assert_eq!(
            w.deposits_once().await.unwrap(),
            0,
            "cursor advanced, nothing new"
        );
        assert_eq!(kinds(&rpc), vec!["register", "deposit:2500000"]);
        assert_eq!(w.state.cursor("ln:invoice_index"), Some(2));
    }

    #[tokio::test]
    async fn pays_only_its_own_assignments_and_reports_success_or_failure() {
        let me = Keypair::from_seed(3).address().to_hex();
        let assignments = vec![
            AssignedPayout {
                outbound_id: 4,
                observer: me.clone(),
                deadline_height: 9,
                fee_allowance: "10".into(),
                invoice: Some("lnbcrt1pay4".into()),
            },
            AssignedPayout {
                outbound_id: 5,
                observer: "cd".repeat(32),
                deadline_height: 9,
                fee_allowance: "10".into(),
                invoice: Some("lnbcrt1pay5".into()),
            },
        ];
        let (w, rpc) = worker(MockLnd {
            settled: Mutex::new(vec![]),
            paid: Mutex::new(vec![]),
            pay_ok: true,
        });
        assert_eq!(w.payouts_once(&assignments).await.unwrap(), 1);
        assert_eq!(
            w.payouts_once(&assignments).await.unwrap(),
            0,
            "reported once"
        );
        assert_eq!(kinds(&rpc), vec!["payout:4:true:2500"]);
        let (w2, rpc2) = worker(MockLnd {
            settled: Mutex::new(vec![]),
            paid: Mutex::new(vec![]),
            pay_ok: false,
        });
        w2.payouts_once(&assignments).await.unwrap();
        assert_eq!(kinds(&rpc2), vec!["payout:4:false:0"]);
    }

    #[tokio::test]
    async fn issues_invoices_bound_to_the_owner() {
        let (w, _) = worker(MockLnd {
            settled: Mutex::new(vec![]),
            paid: Mutex::new(vec![]),
            pay_ok: true,
        });
        let owner = Address::from_hex(&"ab".repeat(32)).unwrap();
        let inv = w.issue_invoice(owner, 1_500).await.unwrap();
        assert!(inv
            .payment_request
            .contains(&format!("keel:{}", owner.to_hex())));
        assert!(inv.payment_request.ends_with("-1500000"));
        assert!(w.issue_invoice(owner, 0).await.is_err());
        assert_eq!(parse_assignments(&json!({ "assignments": [{ "outbound_id": 1, "observer": "aa", "deadline_height": 2, "fee_allowance": "3", "invoice": "x" }] })).len(), 1);
    }

    /// A signed regtest invoice (same builder the chain tests use).
    pub fn real_invoice(memo: &str) -> String {
        use bitcoin::hashes::Hash as _;
        use bitcoin::secp256k1::{Secp256k1, SecretKey};
        use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
        use sha2::Digest as _;
        let secp = Secp256k1::new();
        let sk = SecretKey::from_slice(&[0x11u8; 32]).unwrap();
        let hash: [u8; 32] = sha2::Sha256::digest([7u8; 32]).into();
        InvoiceBuilder::new(Currency::Regtest)
            .description(memo.into())
            .payment_hash(bitcoin::hashes::sha256::Hash::from_slice(&hash).unwrap())
            .payment_secret(PaymentSecret([9u8; 32]))
            .amount_milli_satoshis(2_500_000)
            .current_timestamp()
            .min_final_cltv_expiry_delta(18)
            .build_signed(|h| secp.sign_ecdsa_recoverable(h, &sk))
            .unwrap()
            .to_string()
    }
}
