//! `keel-observer`: the observer-signer daemon. See README.md.
#![forbid(unsafe_code)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use keel_actions::{Action, Chain, VaultRegistration};
use keel_observer::{
    chains::AddressBook,
    config::Config,
    daemon::{keel_rpc, Daemon},
    rpc::{SttRpc, Submitter},
    state::StateFile,
    tss::LocalSigner,
};
use keel_types::Address;
use std::{path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(name = "keel-observer", about = "Keel observer-signer daemon")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon.
    Run {
        #[arg(long)]
        config: PathBuf,
    },
    /// Print an example configuration.
    ExampleConfig,
    /// Print the deposit addresses of a chain's active vault.
    Addresses {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        chain: String,
    },
    /// Submit a `RegisterVault` (devnet helper). With `--local-seed` the
    /// key of the development signer is registered.
    RegisterVault {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        chain: String,
        #[arg(long, default_value = "1")]
        epoch: u64,
        #[arg(long)]
        public_key: Option<String>,
        #[arg(long)]
        chain_code: Option<String>,
        #[arg(long)]
        local_seed: Option<String>,
        /// Observer addresses (hex), comma separated; defaults to this observer.
        #[arg(long, value_delimiter = ',')]
        signers: Option<Vec<String>>,
        #[arg(long, default_value = "1")]
        threshold: u32,
    },
}

fn parse_chain(s: &str) -> anyhow::Result<Chain> {
    keel_observer::rpc::chain_from_json(s)
        .ok_or_else(|| anyhow::anyhow!("unknown chain {s} (BTC, ETH, TRON)"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    match Cli::parse().cmd {
        Cmd::ExampleConfig => print!("{}", keel_observer::config::EXAMPLE),
        Cmd::Run { config } => {
            let cfg = Config::load(&config)?;
            let key = Config::keypair_from_env()?;
            let daemon = Arc::new(Daemon::connect(cfg, key).await?);
            daemon.run().await?;
        }
        Cmd::Addresses { config, chain } => {
            let cfg = Config::load(&config)?;
            let chain = parse_chain(&chain)?;
            let rpc = keel_rpc(&cfg)?;
            let vault = rpc
                .vault(chain)
                .await?
                .context("no vault registered for that chain")?;
            let network = match chain {
                Chain::Bitcoin => cfg
                    .bitcoin
                    .as_ref()
                    .map(|c| c.network)
                    .unwrap_or(keel_chains::Network::Regtest),
                Chain::Ethereum => cfg
                    .ethereum
                    .as_ref()
                    .map(|c| c.network)
                    .unwrap_or(keel_chains::Network::Regtest),
                _ => keel_chains::Network::Mainnet,
            };
            let book = AddressBook::build(vault, network)?;
            for (i, a) in book.addresses() {
                println!(
                    "{i}\t{a}\t{}",
                    book.owner(i)
                        .map(|o| o.to_hex())
                        .unwrap_or_else(|| "-".into())
                );
            }
        }
        Cmd::RegisterVault {
            config,
            chain,
            epoch,
            public_key,
            chain_code,
            local_seed,
            signers,
            threshold,
        } => {
            let cfg = Config::load(&config)?;
            let chain = parse_chain(&chain)?;
            let key = Config::keypair_from_env()?;
            let (pk, cc) = match (local_seed, public_key, chain_code) {
                (Some(seed), _, _) => {
                    let s = LocalSigner::from_seed(&hex::decode(seed.trim())?)?;
                    (s.public_key().to_vec(), s.chain_code())
                }
                (None, Some(pk), Some(cc)) => (
                    hex::decode(pk.trim())?,
                    hex::decode(cc.trim())?
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("chain code must be 32 bytes"))?,
                ),
                _ => anyhow::bail!("pass --local-seed or both --public-key and --chain-code"),
            };
            let signers: Vec<Address> = match signers {
                Some(list) => list
                    .iter()
                    .map(|s| {
                        Address::from_hex(s.trim())
                            .ok_or_else(|| anyhow::anyhow!("bad signer address {s}"))
                    })
                    .collect::<Result<_, _>>()?,
                None => vec![key.address()],
            };
            let rpc: Arc<dyn SttRpc> = Arc::new(keel_rpc(&cfg)?);
            let status = rpc.status().await?;
            let state = Arc::new(StateFile::ephemeral());
            let submitter = Submitter::new(rpc, key, status.chain_id, state);
            let res = submitter
                .submit(
                    None,
                    Action::RegisterVault(VaultRegistration {
                        chain,
                        epoch,
                        public_key: pk,
                        chain_code: Some(cc),
                        signers,
                        threshold,
                    }),
                )
                .await?;
            println!(
                "{}",
                serde_json::json!({ "admitted": res.admitted, "tx_id": res.tx_id, "error": res.error })
            );
        }
    }
    Ok(())
}
