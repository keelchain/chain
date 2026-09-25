//! `keel`: thin CLI over the node's HTTP API. Serde JSON in, HTTP out.
#![forbid(unsafe_code)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use anyhow::{anyhow, bail, Context as _};
use clap::{Parser, Subcommand};
use keel_actions::{
    Action, Bond, HouseQuote, OfferSpec, PlaceOrder, Proposal, ProposalKind, Role, SignedAction,
    StartTrade, Transfer, VoteChoice, Withdraw, CHAIN_ID_DEVNET,
};
use keel_crypto::Keypair;
use keel_types::{Address, Amount, Asset, OrderId, OrderType, Side};
use serde_json::{json, Value};

#[derive(Parser, Debug)]
#[command(name = "keel", about = "Keelchain CLI")]
struct Cli {
    /// Node RPC base URL.
    #[arg(long, global = true, default_value = "http://127.0.0.1:5000")]
    rpc: String,
    #[arg(long, global = true, default_value_t = CHAIN_ID_DEVNET)]
    chain_id: u32,
    #[command(subcommand)]
    cmd: Cmd,
}

mod genesis_export;
mod load;

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Build a genesis file from a ledger export (Phase 6 migration).
    GenesisFromExport(genesis_export::Args),
    /// Load test: fund N accounts from devnet seeds, fire orders, measure.
    Load(load::Args),
    /// Generate a keypair (random, or deterministic from --seed as the devnet does).
    Keygen {
        #[arg(long)]
        seed: Option<u64>,
    },
    /// Node status.
    Status,
    /// Account nonce, budget and balances.
    Account { address: String },
    /// An account's deposit address on an external chain (BTC, ETH, TRON),
    /// if it has requested one (`send deposit-address`).
    DepositAddress { chain: String, address: String },
    /// Order book levels.
    Book {
        #[arg(default_value = "BTC-KUSD")]
        pair: String,
        #[arg(long, default_value_t = 10)]
        depth: usize,
    },
    /// Receipt by tx id.
    Receipt { tx_id: String },
    /// Sign an action and print it as JSON (no submission).
    Sign {
        #[arg(long)]
        secret: String,
        #[arg(long)]
        nonce: Option<u64>,
        #[command(subcommand)]
        action: ActionCmd,
    },
    /// Sign and submit an action; prints the tx id.
    Send {
        #[arg(long)]
        secret: String,
        #[arg(long)]
        nonce: Option<u64>,
        /// Poll until the receipt is available (seconds, 0 = don't wait).
        #[arg(long, default_value_t = 20)]
        wait: u64,
        #[command(subcommand)]
        action: ActionCmd,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ActionCmd {
    /// Ask the chain for a deposit address on an external chain (BTC, ETH, TRON).
    DepositAddress {
        chain: String,
    },
    Transfer {
        to: String,
        asset: String,
        amount: Amount,
        #[arg(long)]
        memo: Option<String>,
    },
    #[command(subcommand)]
    Order(OrderCmd),
    #[command(subcommand)]
    Offer(OfferCmd),
    #[command(subcommand)]
    Trade(TradeCmd),
    Withdraw {
        asset: String,
        to: String,
        amount: Amount,
    },
    #[command(subcommand)]
    Stake(StakeCmd),
    #[command(subcommand)]
    Gov(GovCmd),
    HouseQuote {
        pair: String,
        #[arg(long)]
        bid: Option<String>,
        #[arg(long)]
        ask: Option<String>,
        #[arg(long)]
        valid_until: u64,
    },
    BuyBudget {
        actions: u64,
    },
    /// Lightning pool operations for an observer key (2026-09-10).
    #[command(subcommand)]
    Lightning(LightningCmd),
    /// Raw action JSON (keel_actions::Action serde form).
    Raw {
        json: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum LightningCmd {
    /// Register the observer's LND identity pubkey (33-byte hex).
    Register { node_id: String },
    /// Move vault BTC into this observer's pool: an outbound to `to` (sats).
    Fund { amount: Amount, to: String },
    /// Announce the on-chain tx returning `amount` sats from the pool to the vault.
    Sweep { tx_hash: String, amount: Amount },
}

#[derive(Subcommand, Debug, Clone)]
enum OrderCmd {
    Place {
        pair: String,
        side: String,
        #[arg(long, default_value = "limit")]
        r#type: String,
        #[arg(long)]
        price: Option<Amount>,
        #[arg(long)]
        quantity: Option<Amount>,
        #[arg(long)]
        quote_budget: Option<Amount>,
        #[arg(long)]
        client_id: Option<u64>,
    },
    Cancel {
        order_id: u64,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum OfferCmd {
    /// Create from an OfferSpec JSON file or inline JSON.
    Create {
        spec: String,
    },
    Update {
        offer_id: u64,
        spec: String,
    },
    Pause {
        offer_id: u64,
        #[arg(long)]
        resume: bool,
    },
    Close {
        offer_id: u64,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum TradeCmd {
    Start {
        offer_id: u64,
        amount: Amount,
        fiat_amount: Amount,
        #[arg(long)]
        instructions_hash: Option<String>,
    },
    Paid {
        trade_id: u64,
        #[arg(long)]
        proof_hash: Option<String>,
    },
    Release {
        trade_id: u64,
    },
    Cancel {
        trade_id: u64,
    },
    Dispute {
        trade_id: u64,
        evidence_hash: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum StakeCmd {
    Bond {
        role: String,
        amount: Amount,
        #[arg(long)]
        consensus_key: Option<String>,
    },
    Unbond {
        role: String,
        amount: Amount,
    },
    Delegate {
        validator: String,
        amount: Amount,
    },
    Undelegate {
        validator: String,
        amount: Amount,
    },
    Claim,
}

#[derive(Subcommand, Debug, Clone)]
enum GovCmd {
    /// Propose: kind is a ProposalKind JSON (file or inline).
    Propose {
        title: String,
        kind: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    Vote {
        proposal_id: u64,
        choice: String,
    },
    Execute {
        proposal_id: u64,
    },
}

fn read_json_arg(s: &str) -> anyhow::Result<Value> {
    let text = if std::path::Path::new(s).exists() {
        std::fs::read_to_string(s)?
    } else {
        s.to_string()
    };
    Ok(serde_json::from_str(&text)?)
}

fn parse_hash(s: &str) -> anyhow::Result<[u8; 32]> {
    let v = hex::decode(s)?;
    v.try_into().map_err(|_| anyhow!("hash must be 32 bytes"))
}

fn parse_addr(s: &str) -> anyhow::Result<Address> {
    Address::from_hex(s).ok_or_else(|| anyhow!("bad address {s}"))
}

fn parse_side(s: &str) -> anyhow::Result<Side> {
    match s.to_ascii_lowercase().as_str() {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        _ => bail!("side must be buy|sell"),
    }
}

fn parse_role(s: &str) -> anyhow::Result<Role> {
    match s.to_ascii_lowercase().as_str() {
        "validator" => Ok(Role::Validator),
        "observer" => Ok(Role::Observer),
        "arbitrator" => Ok(Role::Arbitrator),
        _ => bail!("role must be validator|observer|arbitrator"),
    }
}

fn parse_level(s: &str) -> anyhow::Result<(Amount, Amount)> {
    let (p, q) = s
        .split_once('@')
        .ok_or_else(|| anyhow!("level must be <size>@<price>"))?;
    Ok((q.parse()?, p.parse()?))
}

fn build_action(cmd: ActionCmd) -> anyhow::Result<Action> {
    Ok(match cmd {
        ActionCmd::Transfer {
            to,
            asset,
            amount,
            memo,
        } => Action::Transfer(Transfer {
            to: parse_addr(&to)?,
            asset: Asset::new(asset),
            amount,
            memo,
        }),
        ActionCmd::Order(OrderCmd::Place {
            pair,
            side,
            r#type,
            price,
            quantity,
            quote_budget,
            client_id,
        }) => {
            let order_type = match r#type.as_str() {
                "limit" => OrderType::Limit,
                "market" => OrderType::Market,
                _ => bail!("type must be limit|market"),
            };
            Action::PlaceOrder(PlaceOrder {
                pair,
                side: parse_side(&side)?,
                order_type,
                price,
                quantity,
                quote_budget,
                client_id,
            })
        }
        ActionCmd::Order(OrderCmd::Cancel { order_id }) => Action::CancelOrder {
            order_id: OrderId(order_id),
        },
        ActionCmd::Offer(OfferCmd::Create { spec }) => {
            Action::CreateOffer(serde_json::from_value::<OfferSpec>(read_json_arg(&spec)?)?)
        }
        ActionCmd::Offer(OfferCmd::Update { offer_id, spec }) => Action::UpdateOffer {
            offer_id,
            spec: serde_json::from_value(read_json_arg(&spec)?)?,
        },
        ActionCmd::Offer(OfferCmd::Pause { offer_id, resume }) => Action::PauseOffer {
            offer_id,
            paused: !resume,
        },
        ActionCmd::Offer(OfferCmd::Close { offer_id }) => Action::CloseOffer { offer_id },
        ActionCmd::Trade(TradeCmd::Start {
            offer_id,
            amount,
            fiat_amount,
            instructions_hash,
        }) => Action::StartTrade(StartTrade {
            offer_id,
            amount,
            fiat_amount,
            instructions_hash: instructions_hash
                .as_deref()
                .map(parse_hash)
                .transpose()?
                .unwrap_or([0u8; 32]),
        }),
        ActionCmd::Trade(TradeCmd::Paid {
            trade_id,
            proof_hash,
        }) => Action::MarkPaid {
            trade_id,
            proof_hash: proof_hash.as_deref().map(parse_hash).transpose()?,
        },
        ActionCmd::Trade(TradeCmd::Release { trade_id }) => Action::ReleaseTrade { trade_id },
        ActionCmd::Trade(TradeCmd::Cancel { trade_id }) => Action::CancelTrade { trade_id },
        ActionCmd::Trade(TradeCmd::Dispute {
            trade_id,
            evidence_hash,
        }) => Action::OpenDispute {
            trade_id,
            evidence_hash: parse_hash(&evidence_hash)?,
        },
        ActionCmd::DepositAddress { chain } => Action::RequestDepositAddress {
            chain: keel_actions::Chain::parse(&chain)
                .ok_or_else(|| anyhow!("chain must be BTC, ETH or TRON"))?,
        },
        ActionCmd::Withdraw { asset, to, amount } => Action::Withdraw(Withdraw {
            asset: Asset::new(asset),
            to,
            amount,
        }),
        ActionCmd::Stake(StakeCmd::Bond {
            role,
            amount,
            consensus_key,
        }) => Action::Bond(Bond {
            role: parse_role(&role)?,
            amount,
            consensus_key: consensus_key.as_deref().map(parse_hash).transpose()?,
        }),
        ActionCmd::Stake(StakeCmd::Unbond { role, amount }) => Action::Unbond {
            role: parse_role(&role)?,
            amount,
        },
        ActionCmd::Stake(StakeCmd::Delegate { validator, amount }) => Action::Delegate {
            validator: parse_addr(&validator)?,
            amount,
        },
        ActionCmd::Stake(StakeCmd::Undelegate { validator, amount }) => Action::Undelegate {
            validator: parse_addr(&validator)?,
            amount,
        },
        ActionCmd::Stake(StakeCmd::Claim) => Action::ClaimRewards,
        ActionCmd::Lightning(LightningCmd::Register { node_id }) => Action::RegisterLightningNode {
            node_id: hex::decode(node_id.trim_start_matches("0x"))?,
        },
        ActionCmd::Lightning(LightningCmd::Fund { amount, to }) => {
            Action::FundLightningPool { amount, to }
        }
        ActionCmd::Lightning(LightningCmd::Sweep { tx_hash, amount }) => {
            let bytes = hex::decode(tx_hash.trim_start_matches("0x"))?;
            let tx_hash: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("tx_hash must be 32 bytes of hex"))?;
            Action::AnnounceLightningSweep { tx_hash, amount }
        }
        ActionCmd::Gov(GovCmd::Propose {
            title,
            kind,
            description,
        }) => {
            let kind: ProposalKind = serde_json::from_value(read_json_arg(&kind)?)?;
            Action::Propose(Proposal {
                title,
                description,
                kind,
            })
        }
        ActionCmd::Gov(GovCmd::Vote {
            proposal_id,
            choice,
        }) => {
            let choice = match choice.to_ascii_lowercase().as_str() {
                "yes" => VoteChoice::Yes,
                "no" => VoteChoice::No,
                "abstain" => VoteChoice::Abstain,
                "veto" => VoteChoice::Veto,
                _ => bail!("choice must be yes|no|abstain|veto"),
            };
            Action::Vote {
                proposal_id,
                choice,
            }
        }
        ActionCmd::Gov(GovCmd::Execute { proposal_id }) => Action::ExecuteProposal { proposal_id },
        ActionCmd::HouseQuote {
            pair,
            bid,
            ask,
            valid_until,
        } => Action::HouseQuote(HouseQuote {
            pair,
            bid: bid.as_deref().map(parse_level).transpose()?,
            ask: ask.as_deref().map(parse_level).transpose()?,
            valid_until,
        }),
        ActionCmd::BuyBudget { actions } => Action::BuyBudget { actions },
        ActionCmd::Raw { json } => serde_json::from_value(read_json_arg(&json)?)?,
    })
}

struct Client {
    base: String,
}

impl Client {
    fn get(&self, path: &str) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.base, path);
        let mut resp = ureq::get(&url).call().map_err(|e| match e {
            ureq::Error::StatusCode(_) => anyhow!("request failed: {e}"),
            other => anyhow!(other),
        })?;
        Ok(resp.body_mut().read_json::<Value>()?)
    }

    fn get_status(&self, path: &str) -> anyhow::Result<(u16, Value)> {
        let url = format!("{}{}", self.base, path);
        match ureq::get(&url).call() {
            Ok(mut r) => Ok((
                r.status().as_u16(),
                r.body_mut().read_json::<Value>().unwrap_or(Value::Null),
            )),
            Err(ureq::Error::StatusCode(code)) => Ok((code, Value::Null)),
            Err(e) => Err(anyhow!(e)),
        }
    }

    fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.base, path);
        let mut resp = ureq::post(&url)
            .config()
            .http_status_as_error(false)
            .build()
            .send_json(body)?;
        Ok(resp.body_mut().read_json::<Value>()?)
    }

    fn nonce(&self, address: &Address) -> anyhow::Result<u64> {
        let v = self.get(&format!("/v1/accounts/{}", address.to_hex()))?;
        v["nonce"]
            .as_u64()
            .ok_or_else(|| anyhow!("no nonce in account response"))
    }
}

fn keypair(secret: &str) -> anyhow::Result<Keypair> {
    let bytes = hex::decode(secret).context("secret must be hex")?;
    let secret: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("secret must be 32 bytes"))?;
    Ok(Keypair::from_secret(secret))
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = Client {
        base: cli.rpc.trim_end_matches('/').to_string(),
    };
    if let Cmd::GenesisFromExport(args) = &cli.cmd {
        return genesis_export::run(args, cli.chain_id);
    }
    if let Cmd::Load(args) = &cli.cmd {
        return load::run(args, &cli.rpc, cli.chain_id);
    }
    match cli.cmd {
        Cmd::GenesisFromExport(_) | Cmd::Load(_) => unreachable!("handled above"),
        Cmd::Keygen { seed } => {
            let kp = match seed {
                Some(s) => Keypair::from_seed(s),
                None => {
                    let mut secret = [0u8; 32];
                    getrandom::fill(&mut secret).map_err(|e| anyhow::anyhow!("os rng: {e}"))?;
                    Keypair::from_secret(secret)
                }
            };
            println!(
                "{}",
                json!({ "address": kp.address().to_hex(), "secret": hex::encode(kp.secret_bytes()) })
            );
        }
        Cmd::Status => println!(
            "{}",
            serde_json::to_string_pretty(&client.get("/v1/status")?)?
        ),
        Cmd::Account { address } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&client.get(&format!("/v1/accounts/{address}"))?)?
            )
        }
        Cmd::DepositAddress { chain, address } => {
            let chain = keel_actions::Chain::parse(&chain)
                .ok_or_else(|| anyhow!("chain must be BTC, ETH or TRON"))?;
            let v = client.get(&format!(
                "/v1/vaults/{}/addresses?owner={address}",
                chain.as_str()
            ))?;
            match v["addresses"].as_array().and_then(|a| a.first()) {
                Some(row) => println!("{}", serde_json::to_string_pretty(row)?),
                None => bail!("{address} has no {} deposit address yet: run `keel send --secret <secret> deposit-address {}`", chain.as_str(), chain.as_str()),
            }
        }
        Cmd::Book { pair, depth } => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &client.get(&format!("/v1/markets/{pair}/book?depth={depth}"))?
                )?
            )
        }
        Cmd::Receipt { tx_id } => println!(
            "{}",
            serde_json::to_string_pretty(&client.get(&format!("/v1/receipts/{tx_id}"))?)?
        ),
        Cmd::Sign {
            secret,
            nonce,
            action,
        } => {
            let kp = keypair(&secret)?;
            let nonce = match nonce {
                Some(n) => n,
                None => client.nonce(&kp.address())?,
            };
            let sa = SignedAction::sign(&kp, nonce, cli.chain_id, build_action(action)?);
            println!("{}", serde_json::to_string(&sa)?);
        }
        Cmd::Send {
            secret,
            nonce,
            wait,
            action,
        } => {
            let kp = keypair(&secret)?;
            let nonce = match nonce {
                Some(n) => n,
                None => client.nonce(&kp.address())?,
            };
            let sa = SignedAction::sign(&kp, nonce, cli.chain_id, build_action(action)?);
            let resp = client.post("/v1/actions", &serde_json::to_value(&sa)?)?;
            if resp["admitted"] != Value::Bool(true) {
                bail!("refused: {resp}");
            }
            let tx_id = hex::encode(sa.id());
            if wait == 0 {
                println!("{}", json!({ "tx_id": tx_id }));
                return Ok(());
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait);
            loop {
                let (code, v) = client.get_status(&format!("/v1/receipts/{tx_id}"))?;
                if code == 200 {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                    if v["ok"] != Value::Bool(true) {
                        std::process::exit(2);
                    }
                    return Ok(());
                }
                if std::time::Instant::now() > deadline {
                    bail!("timed out waiting for receipt {tx_id}");
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
    }
    Ok(())
}
