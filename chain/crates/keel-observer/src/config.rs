//! TOML configuration of the daemon. Secrets never live in the file:
//! `KEEL_OBSERVER_SECRET` (hex, 32 bytes) is the observer's ed25519 key.

use keel_chains::Network;
use keel_crypto::Keypair;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SECRET_ENV: &str = "KEEL_OBSERVER_SECRET";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Base URL of an Keel node's RPC (`http://127.0.0.1:8545`).
    pub keel_rpc_url: String,
    /// The signer daemon (`http://127.0.0.1:7100`), or `local:<hex seed>`
    /// for a single-key development signer (never in production).
    pub tss_url: String,
    pub state_file: PathBuf,
    #[serde(default)]
    pub intervals: Intervals,
    #[serde(default)]
    pub bitcoin: Option<BitcoinConfig>,
    #[serde(default)]
    pub ethereum: Option<EthereumConfig>,
    #[serde(default)]
    pub tron: Option<TronConfig>,
    #[serde(default)]
    pub outbound: OutboundConfig,
    /// Lightning hot pool via a local LND node (2026-09-10).
    #[serde(default)]
    pub lightning: Option<LightningConfig>,
    /// Devnet only: rebuild the vault view from block receipts while the
    /// node cannot serve `/v1/vaults/*` (see API.md, "Required upstream
    /// changes"). Remove once the node serves the view.
    #[serde(default)]
    pub vault_fallback: Option<VaultFallback>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightningConfig {
    /// LND REST base URL (`https://127.0.0.1:8080`).
    pub rest_url: String,
    #[serde(default)]
    pub macaroon_hex: Option<String>,
    #[serde(default)]
    pub macaroon_path: Option<PathBuf>,
    #[serde(default)]
    pub tls_cert_path: Option<PathBuf>,
    /// Accept LND's self-signed certificate (regtest / local only).
    #[serde(default)]
    pub tls_insecure: bool,
    /// Where the marketplace asks for invoices (`127.0.0.1:7201`); none = no API.
    #[serde(default)]
    pub api_listen: Option<String>,
    #[serde(default = "default_invoice_expiry")]
    pub invoice_expiry_secs: u64,
    #[serde(default = "default_lightning_secs")]
    pub poll_secs: u64,
}

fn default_invoice_expiry() -> u64 {
    3_600
}
fn default_lightning_secs() -> u64 {
    5
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultFallback {
    /// Observer addresses (hex) in the order they were registered as the
    /// vault's signers; the leader of batch `b` is `signers[b % n]`.
    pub signers: Vec<String>,
    #[serde(default = "default_fallback_threshold")]
    pub threshold: u32,
    /// Compressed secp256k1 vault key (hex). Omit with `tss_url =
    /// "local:<seed>"`: the development signer's key is used.
    #[serde(default)]
    pub public_key: Option<String>,
    #[serde(default)]
    pub chain_code: Option<String>,
}

fn default_fallback_threshold() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Intervals {
    /// Refresh vault registrations and deposit-address maps.
    pub sync_secs: u64,
    pub deposits_secs: u64,
    pub outbound_secs: u64,
    pub fees_secs: u64,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            sync_secs: 30,
            deposits_secs: 20,
            outbound_secs: 15,
            fees_secs: 300,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BitcoinConfig {
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_password: String,
    pub network: Network,
    /// Watch-only descriptor wallet the daemon creates/loads.
    #[serde(default = "default_btc_wallet")]
    pub wallet: String,
    /// Most headers attached to one observation (containing block → tip).
    #[serde(default = "default_max_headers")]
    pub max_proof_headers: u64,
    /// `estimatesmartfee` confirmation target.
    #[serde(default = "default_fee_target")]
    pub fee_target_blocks: u32,
    /// Fee rate used when the node has no estimate (regtest).
    #[serde(default = "default_fallback_sat_vb")]
    pub fallback_sat_per_vb: u64,
}

fn default_btc_wallet() -> String {
    "keel-vault-watch".into()
}
fn default_max_headers() -> u64 {
    144
}
fn default_fee_target() -> u32 {
    2
}
fn default_fallback_sat_vb() -> u64 {
    2
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EthereumConfig {
    pub rpc_url: String,
    /// Beacon node REST API (Lighthouse etc.), used for the light-client
    /// proof material. Optional on a devnet where proofs cannot be built.
    #[serde(default)]
    pub beacon_url: Option<String>,
    pub network: Network,
    /// ERC-20 tokens custodied on this chain: KEEL asset symbol → contract.
    #[serde(default)]
    pub tokens: Vec<TokenConfig>,
    /// Blocks scanned per `eth_getLogs` call.
    #[serde(default = "default_log_window")]
    pub log_window: u64,
    /// Priority fee in wei reported/used when `eth_feeHistory` is empty.
    #[serde(default = "default_priority_wei")]
    pub fallback_priority_wei: u128,
    /// Deposit index whose address holds the outbound float (`0` is never
    /// assigned to a user).
    #[serde(default)]
    pub hot_index: u64,
}

fn default_log_window() -> u64 {
    2000
}
fn default_priority_wei() -> u128 {
    1_000_000_000
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenConfig {
    /// Asset symbol on KEEL without the chain prefix (`USDT` → `ETH.USDT`).
    pub symbol: String,
    /// Contract address (`0x…` for Ethereum, `T…` for Tron).
    pub contract: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TronConfig {
    /// Full-node HTTP API base (`https://api.nileex.io`, `http://127.0.0.1:8090`).
    pub api_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub tokens: Vec<TokenConfig>,
    /// Flat network fee reported to the chain, in sun.
    #[serde(default = "default_tron_fee")]
    pub fee_sun: u64,
    /// `fee_limit` for TRC-20 transfers, in sun.
    #[serde(default = "default_tron_fee_limit")]
    pub fee_limit_sun: u64,
    #[serde(default)]
    pub hot_index: u64,
}

fn default_tron_fee() -> u64 {
    15_000_000
}
fn default_tron_fee_limit() -> u64 {
    100_000_000
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OutboundConfig {
    /// Seconds after which a non-leader takes over a batch the leader has
    /// not broadcast.
    pub leader_timeout_secs: u64,
    /// Set to false on a pure observer that never signs.
    pub enabled: bool,
}

impl Default for OutboundConfig {
    fn default() -> Self {
        Self {
            leader_timeout_secs: 900,
            enabled: true,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// The observer key from `KEEL_OBSERVER_SECRET`.
    pub fn keypair_from_env() -> anyhow::Result<Keypair> {
        let s =
            std::env::var(SECRET_ENV).map_err(|_| anyhow::anyhow!("{SECRET_ENV} is not set"))?;
        let bytes = hex::decode(s.trim())?;
        let secret: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("{SECRET_ENV} must be 32 bytes hex"))?;
        Ok(Keypair::from_secret(secret))
    }
}

pub const EXAMPLE: &str = r#"# keel-observer configuration (see README.md)
keel_rpc_url = "http://127.0.0.1:8545"
tss_url = "http://127.0.0.1:7100"          # or "local:<hex seed>" for a dev signer
state_file = "/var/lib/keel-observer/state.json"

[intervals]
sync_secs = 30
deposits_secs = 20
outbound_secs = 15
fees_secs = 300

[bitcoin]
rpc_url = "http://127.0.0.1:18443"
rpc_user = "keel"
rpc_password = "keel"
network = "regtest"                          # mainnet | testnet | signet | regtest
wallet = "keel-vault-watch"
max_proof_headers = 144
fee_target_blocks = 2
fallback_sat_per_vb = 2

[ethereum]
rpc_url = "http://127.0.0.1:8545"
beacon_url = "http://127.0.0.1:5052"       # omit on anvil: no proofs can be built
network = "regtest"
log_window = 2000
hot_index = 0
tokens = [{ symbol = "USDT", contract = "0x5FbDB2315678afecb367f032d93F642f64180aa3" }]

[tron]
api_url = "http://127.0.0.1:8090"
fee_sun = 15000000
fee_limit_sun = 100000000
hot_index = 0
tokens = [{ symbol = "USDT", contract = "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf" }]

[outbound]
enabled = true
leader_timeout_secs = 900

# Devnet only (API.md, "Required upstream changes"): fold block receipts
# into the vault view because /v1/vaults/{chain} answers null.
# [vault_fallback]
# signers = ["<observer 0 hex>", "<observer 1 hex>", "<observer 2 hex>"]
# threshold = 2
# public_key = "02…"      # omit with tss_url = "local:<seed>"
# chain_code = "…"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let c = Config::parse(EXAMPLE).unwrap();
        assert_eq!(c.bitcoin.as_ref().unwrap().network, Network::Regtest);
        assert_eq!(c.ethereum.as_ref().unwrap().tokens[0].symbol, "USDT");
        assert_eq!(c.tron.as_ref().unwrap().fee_sun, 15_000_000);
        assert_eq!(c.intervals.fees_secs, 300);
        assert!(Config::parse("keel_rpc_url = 1").is_err());
        assert!(c.vault_fallback.is_none());
        let with = format!(
            "{EXAMPLE}\n[vault_fallback]\nsigners = [\"{}\"]\n",
            "ab".repeat(32)
        );
        let c = Config::parse(&with).unwrap();
        let f = c.vault_fallback.unwrap();
        assert_eq!(f.signers.len(), 1);
        assert_eq!(f.threshold, 1);
        assert!(f.public_key.is_none());
    }
}
