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

mod genesis_build;
mod genesis_export;
mod load;

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Build a genesis file from a ledger export (Phase 6 migration).
    GenesisFromExport(genesis_export::Args),
    /// Build a real-network genesis from public keys with the roles split
    /// (validators, observers, arbitrators, attesters, param admin, funds).
    GenesisBuild(genesis_build::Args),
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
    /// An account's deposit address on an external chain (BTC, ETH, TRON, BSC, SOL),
    /// if it has requested one (`send deposit-address`).
    DepositAddress { chain: String, address: String },
    /// Order book levels.
    Book {
        #[arg(default_value = "BTC-USDT")]
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
    /// Make sure an account has at least `keel` whole KEEL locked for
    /// action capacity: locks the difference, or does nothing. For service
    /// accounts (a faucet, a validator's operator key) on a network, where
    /// there is no free budget.
    EnsureCapacity {
        #[arg(long)]
        secret: String,
        /// Whole KEEL that must be locked.
        #[arg(long)]
        keel: u64,
        /// Poll until the receipt is available (seconds).
        #[arg(long, default_value_t = 30)]
        wait: u64,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum ActionCmd {
    /// Ask the chain for a deposit address on an external chain (BTC, ETH, TRON, BSC, SOL).
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
    /// Client-owned custody vaults (docs/models.md, Model A).
    #[command(subcommand)]
    Custody(CustodyCmd),
    /// Session keys: a second key that signs a limited set of actions for
    /// this account, with a nonce of its own.
    #[command(subcommand)]
    Session(SessionCmd),
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
    /// Lock KEEL (in orcas) for action capacity; the first lock of an
    /// address is free when it locks at least `budget.free_lock_min_keel`.
    LockBudget {
        amount: Amount,
    },
    /// Start returning locked KEEL (in orcas); it arrives after
    /// `budget.unlock_delay_secs`.
    UnlockBudget {
        amount: Amount,
    },
    /// A client buys a plan period at this tier (1-based), starting now.
    /// Its account must have the tier's KEEL locked and hold the fee in
    /// the stable asset. A higher tier than the running one upgrades.
    BuyPlan {
        tier: u8,
    },
    /// The tier a client's plan renews at when its period ends; 0 stops
    /// renewal.
    SetPlanRenewal {
        tier: u8,
    },
    /// Lightning pool operations for an observer key (2026-09-10).
    #[command(subcommand)]
    Lightning(LightningCmd),
    /// The pooled house-maker inventory (docs/how-it-works.md §11).
    #[command(subcommand)]
    Liquidity(LiquidityCmd),
    /// Raw action JSON (keel_actions::Action serde form).
    Raw {
        json: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum SessionCmd {
    /// Let `key` sign for this account within `--scope`: `markets`,
    /// `p2p`, `client-ops` (a client's service: attestations, custody
    /// addresses, transfers and swaps), `client-payouts` (payouts from the
    /// accounts in its custody), or a sum of their bits.
    Authorize {
        key: String,
        #[arg(long)]
        scope: String,
        /// Seconds from now until the key stops working (the chain caps
        /// it at `sessions.max_secs`, 30 days by default).
        #[arg(long, default_value_t = 29 * 86_400)]
        expires_in_secs: u64,
    },
    /// Stop `key` from signing for this account.
    Revoke { key: String },
}

#[derive(Subcommand, Debug, Clone)]
enum LiquidityCmd {
    /// Fund the pool with `amount` (smallest units) of `asset`; mints shares.
    Deposit { asset: String, amount: Amount },
    /// Burn `shares`; the pro-rata slice of every pool asset is released
    /// after the withdraw delay.
    Withdraw { shares: u128 },
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
enum CustodyCmd {
    /// Register (or rotate, with a higher epoch) this client's vault on a
    /// chain: the vault's compressed public key, its chain code and the
    /// URL of the client's signer.
    Register {
        chain: String,
        #[arg(long, default_value = "1")]
        epoch: u64,
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        chain_code: String,
        #[arg(long)]
        signer_url: String,
    },
    /// Ask for a deposit address inside a client's vault: the signer's
    /// own, or with `--owner` (signed by the custodian) one of the
    /// accounts it attested.
    Address {
        chain: String,
        custodian: String,
        #[arg(long)]
        owner: Option<String>,
    },
    /// Withdraw a custody balance through the custodian's vault.
    Withdraw {
        asset: String,
        to: String,
        amount: Amount,
    },
    /// Swap a balance between this custody and the network vault against
    /// the custodian's floats: `--to deposit` enters the chain's market,
    /// `--to custody` leaves it. No coins move on any external chain.
    Swap {
        asset: String,
        amount: Amount,
        #[arg(long)]
        to: String,
        /// The custodian signs for one of the accounts in its custody.
        #[arg(long)]
        on_behalf_of: Option<String>,
    },
    /// Withdraw unified USDT from this custody on one network: `via` is
    /// that network's USDT (TRON.USDT, BSC.USDT, ETH.USDT), paid out of
    /// the custodian's own vault there. `amount` is in USDT units.
    WithdrawUsdt {
        via: String,
        to: String,
        amount: Amount,
    },
    /// Pay out of an account in your custody, signed by you as its
    /// custodian (or by a key with the `client-payouts` scope). For USDT,
    /// `--via` names the network's USDT to pay on.
    PayOut {
        owner: String,
        asset: String,
        to: String,
        amount: Amount,
        #[arg(long)]
        via: Option<String>,
    },
    /// Move a custody balance to other accounts of the same client (its
    /// escrow account, the buyer, a fee account). No coins move. Several
    /// `--to` are one posting: all of them happen or none.
    Transfer {
        asset: String,
        /// A payment as `<address>=<amount>`; repeat for more legs. As
        /// `<from>:<address>=<amount>` the leg is paid from another
        /// account of the same custodian, which only the custodian signs.
        #[arg(long = "to", required = true)]
        to: Vec<String>,
        /// The custodian signs for one of the accounts in its custody.
        #[arg(long)]
        on_behalf_of: Option<String>,
        /// Your own reference, echoed in the event.
        #[arg(long)]
        memo: Option<String>,
    },
    /// Rotate this client's vault on a chain to a fresh key of its own: the
    /// new key, its chain code and the URL of the signer serving it (the
    /// old signer keeps serving the draining generation).
    Rotate {
        chain: String,
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        chain_code: String,
        #[arg(long)]
        signer_url: String,
        /// Activate at this height instead of at once.
        #[arg(long)]
        activate_at: Option<u64>,
    },
    /// Drop a scheduled rotation before it activates.
    CancelRotation { chain: String },
    /// Retire a draining generation once its grace is over (the custodian
    /// itself, or an observer reporting it empty).
    Retire {
        chain: String,
        custodian: String,
        gen: u64,
    },
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
        ActionCmd::Custody(CustodyCmd::Register {
            chain,
            epoch,
            public_key,
            chain_code,
            signer_url,
        }) => Action::RegisterCustodyVault(keel_actions::CustodyVaultRegistration {
            chain: keel_actions::Chain::parse(&chain)
                .ok_or_else(|| anyhow!("chain must be BTC, ETH or TRON"))?,
            epoch,
            public_key: hex::decode(public_key.trim())?,
            chain_code: Some(
                hex::decode(chain_code.trim())?
                    .try_into()
                    .map_err(|_| anyhow!("chain code must be 32 bytes of hex"))?,
            ),
            signer_url,
        }),
        ActionCmd::Session(SessionCmd::Authorize {
            key,
            scope,
            expires_in_secs,
        }) => Action::AuthorizeSessionKey {
            key: parse_addr(&key)?,
            scope: match scope.to_ascii_lowercase().as_str() {
                "markets" => keel_actions::session_scope::MARKETS,
                "p2p" => keel_actions::session_scope::P2P_MANAGE,
                "client-ops" => keel_actions::session_scope::CLIENT_OPS,
                "client-payouts" => keel_actions::session_scope::CLIENT_PAYOUTS,
                n => n
                    .parse()
                    .map_err(|_| anyhow!("--scope is markets, p2p, client-ops or a number"))?,
            },
            expires_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs()
                + expires_in_secs,
        },
        ActionCmd::Session(SessionCmd::Revoke { key }) => Action::RevokeSessionKey {
            key: parse_addr(&key)?,
        },
        ActionCmd::Custody(CustodyCmd::Address {
            chain,
            custodian,
            owner,
        }) => Action::RequestCustodyAddress {
            chain: keel_actions::Chain::parse(&chain)
                .ok_or_else(|| anyhow!("chain must be BTC, ETH, TRON, BSC or SOL"))?,
            custodian: parse_addr(&custodian)?,
            owner: owner.as_deref().map(parse_addr).transpose()?,
        },
        ActionCmd::Custody(CustodyCmd::Withdraw { asset, to, amount }) => {
            Action::WithdrawCustody(Withdraw {
                asset: Asset::new(asset),
                to,
                amount,
            })
        }
        ActionCmd::Custody(CustodyCmd::PayOut {
            owner,
            asset,
            to,
            amount,
            via,
        }) => Action::WithdrawCustodyFor {
            owner: parse_addr(&owner)?,
            asset: Asset::new(asset),
            via: via.map(Asset::new),
            to,
            amount,
        },
        ActionCmd::Custody(CustodyCmd::WithdrawUsdt { via, to, amount }) => {
            Action::WithdrawCustodyStable(keel_actions::WithdrawStable {
                via: Asset::new(via),
                to,
                amount,
            })
        }
        ActionCmd::Custody(CustodyCmd::Rotate {
            chain,
            public_key,
            chain_code,
            signer_url,
            activate_at,
        }) => Action::RotateCustodyVault(keel_actions::CustodyRotation {
            chain: keel_actions::Chain::parse(&chain)
                .ok_or_else(|| anyhow!("chain must be BTC, ETH or TRON"))?,
            public_key: hex::decode(public_key.trim())?,
            chain_code: hex::decode(chain_code.trim())?
                .try_into()
                .map_err(|_| anyhow!("chain code must be 32 bytes of hex"))?,
            signer_url,
            activate_at,
        }),
        ActionCmd::Custody(CustodyCmd::CancelRotation { chain }) => Action::CancelCustodyRotation {
            chain: keel_actions::Chain::parse(&chain)
                .ok_or_else(|| anyhow!("chain must be BTC, ETH or TRON"))?,
        },
        ActionCmd::Custody(CustodyCmd::Retire {
            chain,
            custodian,
            gen,
        }) => Action::RetireCustodyGeneration {
            chain: keel_actions::Chain::parse(&chain)
                .ok_or_else(|| anyhow!("chain must be BTC, ETH or TRON"))?,
            custodian: parse_addr(&custodian)?,
            gen,
        },
        ActionCmd::Custody(CustodyCmd::Swap {
            asset,
            amount,
            to,
            on_behalf_of,
        }) => Action::SwapCustody(keel_actions::CustodySwap {
            asset: Asset::new(asset),
            amount,
            to: match to.to_ascii_lowercase().as_str() {
                "deposit" => keel_actions::SwapSide::Deposit,
                "custody" => keel_actions::SwapSide::Custody,
                _ => return Err(anyhow!("--to must be deposit or custody")),
            },
            on_behalf_of: on_behalf_of.as_deref().map(parse_addr).transpose()?,
        }),
        ActionCmd::Custody(CustodyCmd::Transfer {
            asset,
            to,
            on_behalf_of,
            memo,
        }) => Action::TransferCustody(keel_actions::CustodyTransfer {
            legs: to
                .iter()
                .map(|leg| {
                    let (addr, amount) = leg
                        .split_once('=')
                        .ok_or_else(|| anyhow!("--to is [<from>:]<address>=<amount>"))?;
                    let (from, addr) = match addr.split_once(':') {
                        Some((from, to)) => (Some(parse_addr(from)?), to),
                        None => (None, addr),
                    };
                    Ok(keel_actions::CustodyLeg {
                        from,
                        to: parse_addr(addr)?,
                        asset: Asset::new(asset.clone()),
                        amount: amount.parse()?,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            on_behalf_of: on_behalf_of.as_deref().map(parse_addr).transpose()?,
            memo,
        }),
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
        ActionCmd::LockBudget { amount } => Action::LockBudget { amount },
        ActionCmd::UnlockBudget { amount } => Action::UnlockBudget { amount },
        ActionCmd::BuyPlan { tier } => Action::BuyPlan { tier },
        ActionCmd::SetPlanRenewal { tier } => Action::SetPlanRenewal { tier },
        ActionCmd::Liquidity(LiquidityCmd::Deposit { asset, amount }) => Action::DepositLiquidity {
            asset: Asset::new(asset),
            amount,
        },
        ActionCmd::Liquidity(LiquidityCmd::Withdraw { shares }) => {
            Action::WithdrawLiquidity { shares }
        }
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

/// Submit a signed action and, when `wait` is above 0, poll for its
/// receipt and print it; exits with status 2 when the action failed.
fn submit(client: &Client, sa: &SignedAction, wait: u64) -> anyhow::Result<()> {
    let resp = client.post("/v1/actions", &serde_json::to_value(sa)?)?;
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = Client {
        base: cli.rpc.trim_end_matches('/').to_string(),
    };
    if let Cmd::GenesisFromExport(args) = &cli.cmd {
        return genesis_export::run(args, cli.chain_id);
    }
    if let Cmd::GenesisBuild(args) = &cli.cmd {
        return genesis_build::run(args, cli.chain_id);
    }
    if let Cmd::Load(args) = &cli.cmd {
        return load::run(args, &cli.rpc, cli.chain_id);
    }
    match cli.cmd {
        Cmd::GenesisFromExport(_) | Cmd::GenesisBuild(_) | Cmd::Load(_) => {
            unreachable!("handled above")
        }
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
            submit(&client, &sa, wait)?;
        }
        Cmd::EnsureCapacity { secret, keel, wait } => {
            let kp = keypair(&secret)?;
            let address = kp.address();
            let account = client.get(&format!("/v1/accounts/{}", address.to_hex()))?;
            let locked: Amount = account["budget"]["locked"]
                .as_str()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let want = Amount::from(keel) * keel_types::ORCAS_PER_KEEL;
            if locked >= want {
                println!(
                    "{}",
                    json!({ "address": address.to_hex(), "locked": locked.to_string(), "changed": false })
                );
                return Ok(());
            }
            let nonce = account["nonce"]
                .as_u64()
                .ok_or_else(|| anyhow!("no nonce in account response"))?;
            let action = Action::LockBudget {
                amount: want - locked,
            };
            let sa = SignedAction::sign(&kp, nonce, cli.chain_id, action);
            submit(&client, &sa, wait.max(1))?;
        }
    }
    Ok(())
}
