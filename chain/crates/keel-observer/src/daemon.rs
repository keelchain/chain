//! Wires the loops: sync (vault registrations → address books), deposits,
//! outbound, fees. Every loop tolerates node errors and retries on its
//! interval.

use crate::{
    chains::{
        btc::{self, BitcoinRpc, HttpBitcoin},
        eth::{BeaconApi, EthRpc, HttpBeacon, HttpEth},
        tron::{HttpTron, TronApi},
        AddressBook,
    },
    config::Config,
    deposits::{self, DepositContext, EthScanner},
    fees, outbound,
    rpc::{ChainParams, FallbackKeys, HttpSttRpc, SttRpc, Submitter},
    state::StateFile,
    tss::{self, LocalSigner, TssClient},
};
use keel_actions::Chain;
use keel_chains::Network;
use keel_crypto::Keypair;
use keel_types::Address;
use std::{sync::Arc, time::Duration};
use tokio::sync::RwLock;

/// Address books per chain, refreshed by the sync loop.
#[derive(Default)]
pub struct Books {
    pub bitcoin: Option<AddressBook>,
    pub ethereum: Option<AddressBook>,
    pub tron: Option<AddressBook>,
    pub params: ChainParams,
}

pub struct Nodes {
    pub lnd: Option<Arc<dyn crate::lightning::LndApi>>,
    pub bitcoin: Option<Arc<dyn BitcoinRpc>>,
    pub ethereum: Option<Arc<dyn EthRpc>>,
    pub beacon: Option<Arc<dyn BeaconApi>>,
    pub tron: Option<Arc<dyn TronApi>>,
}

/// The node client named by the config, with the receipts fallback wired
/// when `[vault_fallback]` is present (its vault key comes from the config
/// or, with `tss_url = "local:<seed>"`, from the development signer).
pub fn keel_rpc(cfg: &Config) -> anyhow::Result<HttpSttRpc> {
    let Some(fb) = &cfg.vault_fallback else {
        return Ok(HttpSttRpc::new(&cfg.keel_rpc_url));
    };
    let (public_key, chain_code) = match (
        &fb.public_key,
        &fb.chain_code,
        cfg.tss_url.strip_prefix("local:"),
    ) {
        (Some(pk), Some(cc), _) => {
            let pk: [u8; 33] = hex::decode(pk.trim())?
                .try_into()
                .map_err(|_| anyhow::anyhow!("vault_fallback.public_key must be 33 bytes"))?;
            let cc: [u8; 32] = hex::decode(cc.trim())?
                .try_into()
                .map_err(|_| anyhow::anyhow!("vault_fallback.chain_code must be 32 bytes"))?;
            (pk, cc)
        }
        (None, None, Some(seed)) => {
            let s = LocalSigner::from_seed(&hex::decode(seed.trim())?)?;
            (s.public_key(), s.chain_code())
        }
        _ => anyhow::bail!(
            "vault_fallback needs public_key and chain_code, or tss_url = \"local:<seed>\""
        ),
    };
    let signers = fb
        .signers
        .iter()
        .map(|s| {
            Address::from_hex(s.trim())
                .ok_or_else(|| anyhow::anyhow!("vault_fallback.signers: bad address {s}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if signers.is_empty() {
        anyhow::bail!("vault_fallback.signers must not be empty");
    }
    tracing::warn!("vault view rebuilt from block receipts (devnet fallback, see API.md)");
    Ok(HttpSttRpc::with_fallback(
        &cfg.keel_rpc_url,
        Some(FallbackKeys {
            public_key,
            chain_code,
            signers,
            threshold: fb.threshold,
        }),
    ))
}

pub struct Daemon {
    pub cfg: Config,
    pub rpc: Arc<dyn SttRpc>,
    pub submitter: Arc<Submitter>,
    pub state: Arc<StateFile>,
    pub tss: Arc<dyn TssClient>,
    pub nodes: Nodes,
    pub books: Arc<RwLock<Books>>,
}

impl Daemon {
    /// Build from the config with real HTTP clients.
    pub async fn connect(cfg: Config, key: Keypair) -> anyhow::Result<Self> {
        let rpc: Arc<dyn SttRpc> = Arc::new(keel_rpc(&cfg)?);
        let status = rpc.status().await?;
        let state = Arc::new(StateFile::load(&cfg.state_file)?);
        let submitter = Arc::new(Submitter::new(
            rpc.clone(),
            key,
            status.chain_id,
            state.clone(),
        ));
        let tss: Arc<dyn TssClient> = Arc::from(tss::client_from_url(&cfg.tss_url)?);
        let nodes = Nodes {
            lnd: match &cfg.lightning {
                Some(c) => Some(Arc::new(crate::lightning::HttpLnd::new(c)?)
                    as Arc<dyn crate::lightning::LndApi>),
                None => None,
            },
            bitcoin: cfg
                .bitcoin
                .as_ref()
                .map(|c| Arc::new(HttpBitcoin::new(c)) as Arc<dyn BitcoinRpc>),
            ethereum: cfg
                .ethereum
                .as_ref()
                .map(|c| Arc::new(HttpEth::new(c)) as Arc<dyn EthRpc>),
            beacon: cfg
                .ethereum
                .as_ref()
                .and_then(|c| c.beacon_url.as_deref())
                .map(|u| Arc::new(HttpBeacon::new(u)) as Arc<dyn BeaconApi>),
            tron: cfg
                .tron
                .as_ref()
                .map(|c| Arc::new(HttpTron::new(c)) as Arc<dyn TronApi>),
        };
        tracing::info!(chain_id = status.chain_id, height = status.height, observer = %submitter.address(), "connected to the Keel node");
        Ok(Self {
            cfg,
            rpc,
            submitter,
            state,
            tss,
            nodes,
            books: Arc::new(RwLock::new(Books::default())),
        })
    }

    /// Refresh vault registrations and derive address books.
    pub async fn sync_once(&self) -> anyhow::Result<()> {
        let params = self.rpc.params().await?;
        let mut books = self.books.write().await;
        books.params = params;
        if let Some(c) = &self.cfg.bitcoin {
            if let Some(v) = self.rpc.vault(Chain::Bitcoin).await? {
                let book = AddressBook::build(v, c.network)?;
                if let Some(rpc) = &self.nodes.bitcoin {
                    let upto = self.state.cursor("btc:imported_upto").unwrap_or(0);
                    match btc::ensure_wallet(
                        rpc.as_ref(),
                        &c.wallet,
                        &book,
                        upto,
                        c.network == Network::Regtest,
                    )
                    .await
                    {
                        Ok(new_upto) => self.state.set_cursor("btc:imported_upto", new_upto)?,
                        Err(e) => tracing::warn!(error = %e, "bitcoin wallet import failed"),
                    }
                }
                books.bitcoin = Some(book);
            }
        }
        if let Some(c) = &self.cfg.ethereum {
            if let Some(v) = self.rpc.vault(Chain::Ethereum).await? {
                books.ethereum = Some(AddressBook::build(v, c.network)?);
            }
        }
        if self.cfg.tron.is_some() {
            if let Some(v) = self.rpc.vault(Chain::Tron).await? {
                books.tron = Some(AddressBook::build(v, Network::Mainnet)?);
            }
        }
        tracing::debug!(
            btc = books.bitcoin.as_ref().map(AddressBook::len),
            eth = books.ethereum.as_ref().map(AddressBook::len),
            tron = books.tron.as_ref().map(AddressBook::len),
            "address books synced"
        );
        Ok(())
    }

    pub async fn deposits_once(&self, eth_scanner: &mut EthScanner) -> anyhow::Result<()> {
        let books = self.books.read().await;
        let ctx = DepositContext {
            submitter: &self.submitter,
            state: &self.state,
            params: &books.params,
        };
        if let (Some(rpc), Some(book), Some(cfg)) =
            (&self.nodes.bitcoin, &books.bitcoin, &self.cfg.bitcoin)
        {
            if let Err(e) = deposits::scan_bitcoin(&ctx, rpc.as_ref(), book, cfg).await {
                tracing::warn!(error = %e, "bitcoin deposit scan failed");
            }
        }
        if let (Some(rpc), Some(book), Some(cfg)) =
            (&self.nodes.ethereum, &books.ethereum, &self.cfg.ethereum)
        {
            let beacon = self.nodes.beacon.as_deref();
            if let Err(e) =
                deposits::scan_ethereum(&ctx, rpc.as_ref(), beacon, eth_scanner, book, cfg).await
            {
                tracing::warn!(error = %e, "ethereum deposit scan failed");
            }
        }
        if let (Some(api), Some(book), Some(cfg)) = (&self.nodes.tron, &books.tron, &self.cfg.tron)
        {
            if let Err(e) = deposits::scan_tron(&ctx, api.as_ref(), book, cfg).await {
                tracing::warn!(error = %e, "tron deposit scan failed");
            }
        }
        Ok(())
    }

    pub async fn outbound_once(&self) -> anyhow::Result<()> {
        let rows = self.rpc.outbounds("Batched").await?;
        if rows.is_empty() {
            return Ok(());
        }
        let books = self.books.read().await;
        let ctx = outbound::OutboundContext {
            submitter: &self.submitter,
            state: &self.state,
            params: &books.params,
            tss: self.tss.as_ref(),
            cfg: &self.cfg.outbound,
        };
        if let (Some(rpc), Some(book), Some(cfg)) =
            (&self.nodes.bitcoin, &books.bitcoin, &self.cfg.bitcoin)
        {
            if let Err(e) = outbound::process_bitcoin(&ctx, rpc.as_ref(), book, cfg, &rows).await {
                tracing::warn!(error = %e, "bitcoin outbound pass failed");
            }
        }
        if let (Some(rpc), Some(book), Some(cfg)) =
            (&self.nodes.ethereum, &books.ethereum, &self.cfg.ethereum)
        {
            if let Err(e) = outbound::process_ethereum(&ctx, rpc.as_ref(), book, cfg, &rows).await {
                tracing::warn!(error = %e, "ethereum outbound pass failed");
            }
        }
        if let (Some(api), Some(book), Some(cfg)) = (&self.nodes.tron, &books.tron, &self.cfg.tron)
        {
            if let Err(e) = outbound::process_tron(&ctx, api.as_ref(), book, cfg, &rows).await {
                tracing::warn!(error = %e, "tron outbound pass failed");
            }
        }
        Ok(())
    }

    pub async fn fees_once(&self) -> anyhow::Result<()> {
        if let (Some(rpc), Some(cfg)) = (&self.nodes.bitcoin, &self.cfg.bitcoin) {
            match fees::report_bitcoin(&self.submitter, rpc.as_ref(), cfg).await {
                Ok(r) => tracing::info!(sat_per_vb = r, "bitcoin fee reported"),
                Err(e) => tracing::warn!(error = %e, "bitcoin fee report failed"),
            }
        }
        if let (Some(rpc), Some(cfg)) = (&self.nodes.ethereum, &self.cfg.ethereum) {
            match fees::report_ethereum(&self.submitter, rpc.as_ref(), cfg).await {
                Ok(r) => tracing::info!(wei_per_gas = r, "ethereum fee reported"),
                Err(e) => tracing::warn!(error = %e, "ethereum fee report failed"),
            }
        }
        if let Some(cfg) = &self.cfg.tron {
            match fees::report_tron(&self.submitter, cfg).await {
                Ok(r) => tracing::info!(sun = r, "tron fee reported"),
                Err(e) => tracing::warn!(error = %e, "tron fee report failed"),
            }
        }
        Ok(())
    }

    /// Run all loops until the process is stopped.
    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        self.sync_once().await?;
        let iv = self.cfg.intervals.clone();
        let sync = {
            let d = self.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(iv.sync_secs.max(1))).await;
                    if let Err(e) = d.sync_once().await {
                        tracing::warn!(error = %e, "sync failed");
                    }
                }
            })
        };
        let deposits = {
            let d = self.clone();
            tokio::spawn(async move {
                let mut scanner = EthScanner::default();
                loop {
                    if let Err(e) = d.deposits_once(&mut scanner).await {
                        tracing::warn!(error = %e, "deposit pass failed");
                    }
                    tokio::time::sleep(Duration::from_secs(iv.deposits_secs.max(1))).await;
                }
            })
        };
        let outbound = {
            let d = self.clone();
            tokio::spawn(async move {
                loop {
                    if let Err(e) = d.outbound_once().await {
                        tracing::warn!(error = %e, "outbound pass failed");
                    }
                    tokio::time::sleep(Duration::from_secs(iv.outbound_secs.max(1))).await;
                }
            })
        };
        let fees = {
            let d = self.clone();
            tokio::spawn(async move {
                loop {
                    if let Err(e) = d.fees_once().await {
                        tracing::warn!(error = %e, "fee pass failed");
                    }
                    tokio::time::sleep(Duration::from_secs(iv.fees_secs.max(1))).await;
                }
            })
        };
        // Lightning (2026-09-10): register, report settled invoices,
        // pay assigned payouts; plus the invoice API for the marketplace.
        let lightning = {
            let d = self.clone();
            tokio::spawn(async move {
                let (Some(lnd), Some(cfg)) = (d.nodes.lnd.clone(), d.cfg.lightning.clone()) else {
                    std::future::pending::<()>().await;
                    return;
                };
                let worker = Arc::new(crate::lightning::LightningWorker {
                    lnd,
                    submitter: d.submitter.clone(),
                    state: d.state.clone(),
                    rpc: d.rpc.clone(),
                    invoice_expiry_secs: cfg.invoice_expiry_secs,
                });
                if let Some(listen) = cfg.api_listen.clone() {
                    let w = worker.clone();
                    tokio::spawn(async move {
                        if let Err(e) = crate::lightning::serve_api(&listen, w).await {
                            tracing::error!(error = %e, "lightning invoice API failed");
                        }
                    });
                }
                loop {
                    if let Err(e) = worker.register_once().await {
                        tracing::warn!(error = %e, "lightning register failed");
                    }
                    if let Err(e) = worker.deposits_once().await {
                        tracing::warn!(error = %e, "lightning deposit pass failed");
                    }
                    match d.rpc.lightning().await {
                        Ok(v) => {
                            let assignments = crate::lightning::parse_assignments(&v);
                            if let Err(e) = worker.payouts_once(&assignments).await {
                                tracing::warn!(error = %e, "lightning payout pass failed");
                            }
                        }
                        Err(e) => tracing::warn!(error = %e, "lightning assignments fetch failed"),
                    }
                    tokio::time::sleep(Duration::from_secs(cfg.poll_secs.max(1))).await;
                }
            })
        };
        tokio::select! {
            r = sync => r?,
            r = deposits => r?,
            r = outbound => r?,
            r = fees => r?,
            r = lightning => r?,
            _ = tokio::signal::ctrl_c() => tracing::info!("stopping"),
        }
        Ok(())
    }
}
