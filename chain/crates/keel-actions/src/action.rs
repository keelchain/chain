//! The complete action set, grouped by module. Every variant is applied by
//! `keel-vm`; this file is the wire contract for the SDK, RPC and observers.

use borsh::{BorshDeserialize, BorshSerialize};
use keel_crypto::Hash32;
use keel_types::{Address, Amount, Asset, OrderId, OrderType, PairConfig, Side};
use serde::{Deserialize, Serialize};

pub type OfferId = u64;
pub type TradeId = u64;
pub type ProposalId = u64;
pub type OutboundId = u64;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Action {
    // ---- tokens
    Transfer(Transfer),
    // ---- markets
    PlaceOrder(PlaceOrder),
    CancelOrder {
        order_id: OrderId,
    },
    /// Signed two-sided quote from the house maker (or any account holding
    /// a `swap_pool`-style inventory the pair config names as house party).
    HouseQuote(HouseQuote),
    // ---- budgets
    BuyBudget {
        actions: u64,
    },
    // ---- P2P offers and trades
    CreateOffer(OfferSpec),
    UpdateOffer {
        offer_id: OfferId,
        spec: OfferSpec,
    },
    PauseOffer {
        offer_id: OfferId,
        paused: bool,
    },
    CloseOffer {
        offer_id: OfferId,
    },
    StartTrade(StartTrade),
    MarkPaid {
        trade_id: TradeId,
        #[serde(with = "crate::hex32::option")]
        proof_hash: Option<Hash32>,
    },
    ReleaseTrade {
        trade_id: TradeId,
    },
    CancelTrade {
        trade_id: TradeId,
    },
    OpenDispute {
        trade_id: TradeId,
        #[serde(with = "crate::hex32")]
        evidence_hash: Hash32,
    },
    SubmitEvidence {
        trade_id: TradeId,
        #[serde(with = "crate::hex32")]
        evidence_hash: Hash32,
    },
    RuleDispute {
        trade_id: TradeId,
        ruling: Ruling,
    },
    // ---- vaults (cross-chain custody)
    RequestDepositAddress {
        chain: Chain,
    },
    ObserveDeposit(DepositObservation),
    ObserveOutbound(OutboundObservation),
    ReportNetworkFee {
        chain: Chain,
        fee_rate: u64,
    },
    Withdraw(Withdraw),
    RegisterVault(VaultRegistration),
    // ---- stablecoin
    MintStable {
        asset: Asset,
        amount: Amount,
    },
    BurnStable {
        asset: Asset,
        amount: Amount,
    },
    // ---- staking
    Bond(Bond),
    Unbond {
        role: Role,
        amount: Amount,
    },
    Delegate {
        validator: Address,
        amount: Amount,
    },
    Undelegate {
        validator: Address,
        amount: Amount,
    },
    ClaimRewards,
    // ---- governance
    Propose(Proposal),
    Vote {
        proposal_id: ProposalId,
        choice: VoteChoice,
    },
    ExecuteProposal {
        proposal_id: ProposalId,
    },
    // ---- attestations (KYC tiers etc.) by governance-registered attesters
    Attest {
        subject: Address,
        tier: u8,
        expires_at: u64,
    },
    /// Direct parameter change by the governance-appointed `param_admin`
    /// (the platform's super admin at launch). Same keys as `ParamChange`;
    /// bypasses voting until governance revokes the admin.
    SetParam {
        key: String,
        value: u128,
    },
    // ---- action capacity by locking KEEL (Tron energy model, 2026-09-08)
    /// Lock KEEL from the deposit account; grants `per_locked_keel_per_day`
    /// actions per whole KEEL per day on top of the free budget.
    LockBudget {
        amount: Amount,
    },
    /// Queue KEEL to return to the deposit account after `unlock_delay_secs`.
    UnlockBudget {
        amount: Amount,
    },
    // ---- session keys (2026-09-09, non-custodial wallets): a
    // scope-limited key a site may hold so trading needs no popup per click.
    /// Signed by the principal. `scope` is a bitset (`session_scope`);
    /// `expires_at` is block time in seconds.
    AuthorizeSessionKey {
        key: Address,
        scope: u32,
        expires_at: u64,
    },
    /// By the principal or the session key itself.
    RevokeSessionKey {
        key: Address,
    },
    // ---- Lightning (2026-09-10): per-observer hot pools next to the
    // cold vault. All limits are chain params (backoffice-editable).
    /// Observer registers (or rotates) its Lightning node id (33-byte
    /// compressed secp256k1 pubkey); invoices it issues must be signed by it.
    RegisterLightningNode {
        node_id: Vec<u8>,
    },
    /// Observer reports a settled deposit invoice it issued: the preimage
    /// proves settlement, the description binds the owner.
    ObserveLightningDeposit(LightningDepositObservation),
    /// Observer assigned to a Lightning payout reports the result.
    ObserveLightningPayout {
        outbound_id: OutboundId,
        preimage: Option<Hash32>,
        fee_paid_msat: u64,
        success: bool,
    },
    /// Observer moves vault BTC into its Lightning pool: a normal outbound
    /// from the vault to the observer's on-chain Lightning wallet.
    FundLightningPool {
        amount: Amount,
        to: String,
    },
    /// Observer announces the on-chain tx that returns pool BTC to the
    /// vault (the tx is then observed as a normal deposit to index 0).
    AnnounceLightningSweep {
        tx_hash: Hash32,
        amount: Amount,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct LightningDepositObservation {
    /// The BOLT11 invoice string the observer issued.
    pub invoice: String,
    #[serde(with = "crate::hex32")]
    pub preimage: Hash32,
    /// Amount actually received, in millisatoshis.
    pub amount_msat: u64,
}

/// Session scope bits. Anything not listed can never be signed by a
/// session key: transfers, withdrawals, escrow release/cancel/start,
/// budget locks, staking, governance, attestations, session management.
pub mod session_scope {
    /// PlaceOrder, CancelOrder, HouseQuote.
    pub const MARKETS: u32 = 1;
    /// CreateOffer, UpdateOffer, PauseOffer, CloseOffer, MarkPaid.
    pub const P2P_MANAGE: u32 = 2;
    pub const ALL: u32 = MARKETS | P2P_MANAGE;
}

impl Action {
    /// The scope bit a session key needs to sign this action; `None` when
    /// no session key may ever sign it.
    pub fn session_scope(&self) -> Option<u32> {
        match self {
            Action::PlaceOrder(_) | Action::CancelOrder { .. } | Action::HouseQuote(_) => {
                Some(session_scope::MARKETS)
            }
            Action::CreateOffer(_)
            | Action::UpdateOffer { .. }
            | Action::PauseOffer { .. }
            | Action::CloseOffer { .. }
            | Action::MarkPaid { .. } => Some(session_scope::P2P_MANAGE),
            _ => None,
        }
    }

    /// Cancels get the wider budget so a capped address can always unwind.
    pub fn is_cancel(&self) -> bool {
        matches!(
            self,
            Action::CancelOrder { .. } | Action::CloseOffer { .. } | Action::CancelTrade { .. }
        )
    }

    /// Actions only bonded observers may submit; they are budget-free.
    pub fn is_observer_action(&self) -> bool {
        matches!(
            self,
            Action::ObserveDeposit(_)
                | Action::ObserveOutbound(_)
                | Action::ReportNetworkFee { .. }
                | Action::RegisterVault(_)
                | Action::RegisterLightningNode { .. }
                | Action::ObserveLightningDeposit(_)
                | Action::ObserveLightningPayout { .. }
                | Action::FundLightningPool { .. }
                | Action::AnnounceLightningSweep { .. }
        )
    }

    pub fn module(&self) -> &'static str {
        match self {
            Action::Transfer(_) => "tokens",
            Action::PlaceOrder(_) | Action::CancelOrder { .. } | Action::HouseQuote(_) => "markets",
            Action::BuyBudget { .. } | Action::LockBudget { .. } | Action::UnlockBudget { .. } => {
                "budgets"
            }
            Action::AuthorizeSessionKey { .. } | Action::RevokeSessionKey { .. } => "sessions",
            Action::RegisterLightningNode { .. }
            | Action::ObserveLightningDeposit(_)
            | Action::ObserveLightningPayout { .. }
            | Action::FundLightningPool { .. }
            | Action::AnnounceLightningSweep { .. } => "lightning",
            Action::CreateOffer(_)
            | Action::UpdateOffer { .. }
            | Action::PauseOffer { .. }
            | Action::CloseOffer { .. }
            | Action::StartTrade(_)
            | Action::MarkPaid { .. }
            | Action::ReleaseTrade { .. }
            | Action::CancelTrade { .. } => "market_p2p",
            Action::OpenDispute { .. }
            | Action::SubmitEvidence { .. }
            | Action::RuleDispute { .. } => "disputes",
            Action::RequestDepositAddress { .. }
            | Action::ObserveDeposit(_)
            | Action::ObserveOutbound(_)
            | Action::ReportNetworkFee { .. }
            | Action::Withdraw(_)
            | Action::RegisterVault(_) => "vaults",
            Action::MintStable { .. } | Action::BurnStable { .. } => "stable",
            Action::Bond(_)
            | Action::Unbond { .. }
            | Action::Delegate { .. }
            | Action::Undelegate { .. }
            | Action::ClaimRewards => "staking",
            Action::Propose(_) | Action::Vote { .. } | Action::ExecuteProposal { .. } => "gov",
            Action::Attest { .. } => "attest",
            Action::SetParam { .. } => "gov",
        }
    }
}

// ---------------- tokens ----------------

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Transfer {
    pub to: Address,
    pub asset: Asset,
    pub amount: Amount,
    pub memo: Option<String>,
}

// ---------------- markets ----------------

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct PlaceOrder {
    pub pair: String,
    pub side: Side,
    pub order_type: OrderType,
    pub price: Option<Amount>,
    pub quantity: Option<Amount>,
    /// Market buy by quote budget ("spend this much quote").
    pub quote_budget: Option<Amount>,
    /// Client reference echoed in events; not unique on chain.
    pub client_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct HouseQuote {
    pub pair: String,
    /// (price, size) the house is willing to BUY base at.
    pub bid: Option<(Amount, Amount)>,
    /// (price, size) the house is willing to SELL base at.
    pub ask: Option<(Amount, Amount)>,
    /// Block height after which the quote is void.
    pub valid_until: u64,
}

// ---------------- P2P offers and trades ----------------

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct OfferSpec {
    pub side: Side,
    /// The crypto asset offered (sell) or wanted (buy).
    pub asset: Asset,
    pub fiat_currency: String,
    pub payment_method: String,
    /// Margin over the oracle spot in basis points (may be negative).
    pub margin_bps: i32,
    /// Fixed fiat price per whole asset unit, in fiat minor units, if the
    /// maker does not want to float on the oracle.
    pub fixed_price: Option<Amount>,
    pub min_amount: Amount,
    pub max_amount: Amount,
    /// Seconds the buyer has to pay after the trade starts.
    pub payment_window_secs: u32,
    pub country: Option<String>,
    /// Minimum attestation tier the counterparty must hold.
    pub min_tier: u8,
    pub terms: String,
    /// Hash of the encrypted payment instructions kept off chain.
    #[serde(with = "crate::hex32")]
    pub instructions_hash: Hash32,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct StartTrade {
    pub offer_id: OfferId,
    /// Crypto amount in smallest units.
    pub amount: Amount,
    /// Fiat amount in minor units the taker will pay/receive; the VM checks
    /// it against the offer's price at start.
    pub fiat_amount: Amount,
    /// Hash of the taker's encrypted contact/instructions blob.
    #[serde(with = "crate::hex32")]
    pub instructions_hash: Hash32,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum Ruling {
    WinsSeller,
    WinsBuyer,
    /// Buyer receives `buyer_bps` of the escrow, seller the rest.
    Split {
        buyer_bps: u16,
    },
}

// ---------------- vaults ----------------

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum Chain {
    Bitcoin,
    Ethereum,
    Tron,
}

impl Chain {
    pub fn as_str(self) -> &'static str {
        match self {
            Chain::Bitcoin => "BTC",
            Chain::Ethereum => "ETH",
            Chain::Tron => "TRON",
        }
    }

    pub fn parse(s: &str) -> Option<Chain> {
        match s {
            "BTC" => Some(Chain::Bitcoin),
            "ETH" => Some(Chain::Ethereum),
            "TRON" => Some(Chain::Tron),
            _ => None,
        }
    }

    /// Confirmation depth before a deposit may be credited (inherited from
    /// the custody gateway; governance can raise it).
    pub fn default_confirmations(self) -> u32 {
        match self {
            Chain::Bitcoin => 2,
            Chain::Ethereum => 12,
            Chain::Tron => 19,
        }
    }

    /// Whether non-observer validators can verify a light-client proof.
    pub fn has_light_client(self) -> bool {
        matches!(self, Chain::Bitcoin | Chain::Ethereum)
    }
}

/// Light-client evidence carried with an observation. Verified by
/// `keel-lc-btc` / `keel-lc-eth`; `None` for attestation-only chains.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum Proof {
    None,
    /// Raw 80-byte headers from the confirmed block onward plus a partial
    /// merkle tree proving the txid is in the first header.
    Bitcoin {
        headers: Vec<Vec<u8>>,
        merkle_proof: Vec<u8>,
        tx_index: u32,
    },
    /// Finalized beacon header + execution receipt proof (opaque bytes for
    /// the eth light-client crate).
    Ethereum {
        proof: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct DepositObservation {
    pub chain: Chain,
    pub asset: Asset,
    #[serde(with = "crate::hex32")]
    pub tx_hash: Hash32,
    /// Output index (BTC) or log index (EVM/Tron).
    pub index: u32,
    /// Deposit-address index the funds landed on (maps to an owner on chain).
    pub deposit_index: u64,
    pub amount: Amount,
    /// Height of the external block containing the tx.
    pub external_height: u64,
    /// Current tip height on the observer's node (for depth).
    pub tip_height: u64,
    pub proof: Proof,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct OutboundObservation {
    pub outbound_id: OutboundId,
    #[serde(with = "crate::hex32")]
    pub tx_hash: Hash32,
    pub external_height: u64,
    pub tip_height: u64,
    /// Network fee actually paid, in the chain's native smallest units.
    pub fee_paid: Amount,
    pub success: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Withdraw {
    pub asset: Asset,
    /// Destination address on the home chain, as a string in that chain's
    /// canonical form.
    pub to: String,
    pub amount: Amount,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct VaultRegistration {
    pub chain: Chain,
    pub epoch: u64,
    /// Compressed secp256k1 (33 bytes) or ed25519 (32 bytes) vault key.
    pub public_key: Vec<u8>,
    /// Chain code for non-hardened child derivation (secp256k1 vaults).
    #[serde(with = "crate::hex32::option")]
    pub chain_code: Option<[u8; 32]>,
    /// Observer-signers holding shares of this key.
    pub signers: Vec<Address>,
    pub threshold: u32,
}

// ---------------- staking ----------------

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub enum Role {
    Validator,
    Observer,
    Arbitrator,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Bond {
    pub role: Role,
    pub amount: Amount,
    /// ed25519 consensus key for validators (may equal the account key).
    #[serde(with = "crate::hex32::option")]
    pub consensus_key: Option<[u8; 32]>,
}

// ---------------- governance ----------------

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Proposal {
    pub title: String,
    pub description: String,
    pub kind: ProposalKind,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub enum ProposalKind {
    /// Numeric parameter change; keys are listed in `keel_vm::params`.
    ParamChange {
        key: String,
        value: u128,
    },
    ListPair(PairConfig),
    DelistPair {
        symbol: String,
    },
    RegisterAsset {
        asset: Asset,
        decimals: u32,
    },
    TreasurySpend {
        to: Address,
        asset: Asset,
        amount: Amount,
    },
    SoftwareUpgrade {
        version: String,
        height: u64,
    },
    SetArbitrators {
        members: Vec<Address>,
    },
    SetAttesters {
        members: Vec<Address>,
    },
    SetObservers {
        members: Vec<Address>,
        threshold: u32,
    },
    /// Stablecoin basket membership and cap per reserve asset.
    SetStableBasket {
        asset: Asset,
        cap: Amount,
        enabled: bool,
    },
    PauseModule {
        module: String,
        until_height: u64,
    },
    Text,
    /// Trusted Bitcoin header checkpoint the SPV verifier extends from.
    SetBtcCheckpoint(BtcCheckpoint),
    /// Ethereum sync-committee bootstrap for the light client.
    SetEthCheckpoint(EthCheckpoint),
    /// ERC-20 / TRC-20 contract behind a vault asset (20 bytes).
    SetTokenContract {
        asset: Asset,
        contract: Vec<u8>,
    },
    /// Appoint (or with `None` revoke) the address allowed to `SetParam`.
    SetParamAdmin {
        admin: Option<Address>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BtcCheckpoint {
    /// 0 mainnet, 1 testnet, 2 signet, 3 regtest.
    pub network: u8,
    pub height: u64,
    /// 80-byte header at `height`.
    pub header: Vec<u8>,
    /// Timestamp of the first block of the retarget period containing
    /// `height` (needed to check the next retarget).
    pub period_start_time: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct EthCheckpoint {
    pub period: u64,
    #[serde(with = "crate::hex32")]
    pub committee_root: [u8; 32],
    #[serde(with = "crate::hex32::option")]
    pub next_committee_root: Option<[u8; 32]>,
    #[serde(with = "crate::hex32")]
    pub genesis_validators_root: [u8; 32],
    pub fork_version: [u8; 4],
    pub committee_size: u32,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub enum VoteChoice {
    Yes,
    No,
    Abstain,
    /// No with veto: enough of these burns the deposit.
    Veto,
}
