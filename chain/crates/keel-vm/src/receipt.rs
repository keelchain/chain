use borsh::{BorshDeserialize, BorshSerialize};
use keel_ledger::LedgerError;
use keel_types::{Address, Amount, Asset};
use serde::{Deserialize, Serialize};

/// Why an action was refused. Stable codes; messages are for humans.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    thiserror::Error,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub enum VmError {
    #[error("bad signature")]
    BadSignature,
    #[error("wrong chain id")]
    WrongChain,
    #[error("bad nonce: expected {expected}, got {got}")]
    BadNonce { expected: u64, got: u64 },
    #[error("action budget exhausted")]
    BudgetExhausted,
    #[error("per-block action cap reached")]
    BlockCap,
    #[error("module {0} is paused")]
    Paused(String),
    #[error("not authorized")]
    Unauthorized,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("not enough funds")]
    NotEnoughFunds,
    #[error("ledger: {0}")]
    Ledger(String),
    #[error("unimplemented: {0}")]
    Unimplemented(String),
}

impl From<LedgerError> for VmError {
    fn from(e: LedgerError) -> Self {
        match e {
            LedgerError::NotEnoughFunds { .. } => VmError::NotEnoughFunds,
            other => VmError::Ledger(other.to_string()),
        }
    }
}

impl VmError {
    pub fn code(&self) -> &'static str {
        match self {
            VmError::BadSignature => "BAD_SIGNATURE",
            VmError::WrongChain => "WRONG_CHAIN",
            VmError::BadNonce { .. } => "BAD_NONCE",
            VmError::BudgetExhausted => "BUDGET_EXHAUSTED",
            VmError::BlockCap => "BLOCK_CAP",
            VmError::Paused(_) => "PAUSED",
            VmError::Unauthorized => "UNAUTHORIZED",
            VmError::NotFound(_) => "NOT_FOUND",
            VmError::Invalid(_) => "INVALID",
            VmError::NotEnoughFunds => "NOT_ENOUGH_FUNDS",
            VmError::Ledger(_) => "LEDGER",
            VmError::Unimplemented(_) => "UNIMPLEMENTED",
        }
    }
}

/// Something a client may want to index. Kept small and flat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub enum Event {
    Transferred {
        from: Address,
        to: Address,
        asset: Asset,
        amount: Amount,
    },
    OrderAccepted {
        order_id: u64,
        owner: Address,
        pair: String,
        resting: Amount,
    },
    OrderFilled {
        order_id: u64,
        maker_order_id: Option<u64>,
        pair: String,
        price: Amount,
        quantity: Amount,
        quote: Amount,
        fee: Amount,
    },
    OrderCancelled {
        order_id: u64,
        released: Amount,
    },
    OrderRejected {
        pair: String,
        reason: String,
    },
    BudgetPurchased {
        owner: Address,
        actions: u64,
        paid: Amount,
    },
    BudgetLocked {
        owner: Address,
        amount: Amount,
        locked_total: Amount,
    },
    BudgetUnlockQueued {
        owner: Address,
        amount: Amount,
        ready_at: u64,
    },
    BudgetUnlocked {
        owner: Address,
        amount: Amount,
    },
    OfferCreated {
        offer_id: u64,
        owner: Address,
    },
    OfferUpdated {
        offer_id: u64,
    },
    OfferClosed {
        offer_id: u64,
    },
    TradeStarted {
        trade_id: u64,
        offer_id: u64,
        buyer: Address,
        seller: Address,
        amount: Amount,
    },
    TradePaid {
        trade_id: u64,
    },
    TradeReleased {
        trade_id: u64,
        to_buyer: Amount,
        fee: Amount,
    },
    TradeCancelled {
        trade_id: u64,
        by: Address,
    },
    DisputeOpened {
        trade_id: u64,
        by: Address,
    },
    DisputeRuled {
        trade_id: u64,
        buyer_amount: Amount,
        seller_amount: Amount,
    },
    DepositAddressAssigned {
        owner: Address,
        chain: String,
        index: u64,
    },
    DepositObserved {
        chain: String,
        tx_hash: String,
        votes: u32,
    },
    DepositCredited {
        owner: Address,
        asset: Asset,
        amount: Amount,
    },
    WithdrawalQueued {
        outbound_id: u64,
        owner: Address,
        asset: Asset,
        amount: Amount,
        to: String,
    },
    OutboundBatched {
        outbound_id: u64,
        chain: String,
    },
    OutboundConfirmed {
        outbound_id: u64,
        tx_hash: String,
    },
    VaultRegistered {
        chain: String,
        epoch: u64,
    },
    StableMinted {
        owner: Address,
        from: Asset,
        amount: Amount,
    },
    StableBurned {
        owner: Address,
        into: Asset,
        amount: Amount,
    },
    Bonded {
        owner: Address,
        role: String,
        amount: Amount,
    },
    Unbonded {
        owner: Address,
        role: String,
        amount: Amount,
        at_height: u64,
    },
    Delegated {
        owner: Address,
        validator: Address,
        amount: Amount,
    },
    RewardsClaimed {
        owner: Address,
        asset: Asset,
        amount: Amount,
    },
    Slashed {
        owner: Address,
        amount: Amount,
        reason: String,
    },
    ProposalCreated {
        proposal_id: u64,
        proposer: Address,
    },
    Voted {
        proposal_id: u64,
        voter: Address,
        weight: Amount,
    },
    ProposalExecuted {
        proposal_id: u64,
        ok: bool,
    },
    ParamChanged {
        key: String,
        value: u128,
    },
    Attested {
        subject: Address,
        tier: u8,
    },
    EpochAdvanced {
        epoch: u64,
        validators: u32,
    },
    InvariantBreached {
        asset: Asset,
        reserves: Amount,
        liabilities: Amount,
    },
    Undelegated {
        owner: Address,
        validator: Address,
        amount: Amount,
        at_height: u64,
    },
    BondReleased {
        owner: Address,
        role: String,
        amount: Amount,
    },
    DepositHeld {
        owner: Address,
        asset: Asset,
        amount: Amount,
        release_height: u64,
    },
    DepositReleased {
        owner: Address,
        asset: Asset,
        amount: Amount,
    },
    OutboundFailed {
        outbound_id: u64,
        refunded: Amount,
    },
    NetworkFeeReported {
        chain: String,
        observer: Address,
        fee_rate: u64,
    },
    OutboundsHalted {
        asset: Asset,
    },
    OutboundsResumed {
        asset: Asset,
    },
    ProposalTallied {
        proposal_id: u64,
        status: String,
    },
    RewardsDistributed {
        epoch: u64,
        asset: Asset,
        amount: Amount,
    },
    ParamAdminChanged {
        admin: Option<Address>,
    },
    SessionAuthorized {
        principal: Address,
        key: Address,
        scope: u32,
        expires_at: u64,
    },
    SessionRevoked {
        principal: Address,
        key: Address,
    },
    LightningNodeRegistered {
        observer: Address,
        node_id: String,
    },
    LightningDepositCredited {
        owner: Address,
        observer: Address,
        amount: Amount,
        payment_hash: String,
    },
    LightningPayoutAssigned {
        outbound_id: u64,
        observer: Address,
    },
    LightningPayoutSettled {
        outbound_id: u64,
        observer: Address,
        fee_paid: Amount,
    },
    LightningPayoutFailed {
        outbound_id: u64,
        refunded: Amount,
    },
    LightningPoolFunded {
        observer: Address,
        amount: Amount,
        outbound_id: u64,
    },
    LightningSweepAnnounced {
        observer: Address,
        tx_hash: String,
        amount: Amount,
    },
    LightningPoolSwept {
        observer: Address,
        amount: Amount,
    },
    VestingReleased {
        owner: Address,
        amount: Amount,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Receipt {
    pub index: u32,
    /// Block that applied the action and its BFT timestamp (ms).
    pub height: u64,
    pub timestamp: u64,
    pub tx_id: [u8; 32],
    pub signer: Address,
    pub ok: bool,
    pub error: Option<VmError>,
    pub events: Vec<Event>,
}
