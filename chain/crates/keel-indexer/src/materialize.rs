//! Pure block → rows materialization. No I/O: the DB layer preloads a
//! [`Ctx`] with the records a block refers to (orders, outbounds, trades…)
//! and applies the returned rows and patches in one transaction.

use crate::types::{
    addresses_in, amount_of, field_addr, field_amount, field_str, field_u64, split_event,
    ActionRow, Amount, BlockData,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const SYSTEM_ADDR: &str = "0000000000000000000000000000000000000000000000000000000000000000";

// ---------------- lookup context ----------------

#[derive(Clone, Debug, Default)]
pub struct OrderInfo {
    pub owner: String,
    pub pair: String,
    pub side: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct OutboundInfo {
    pub owner: String,
    pub asset: String,
    pub amount: Amount,
}

#[derive(Clone, Debug, Default)]
pub struct TradeInfo {
    pub buyer: String,
    pub seller: String,
    pub asset: String,
    pub amount: Amount,
}

#[derive(Clone, Debug, Default)]
pub struct OfferInfo {
    pub owner: String,
    pub asset: String,
    pub fiat_currency: Option<String>,
}

/// Records a batch refers to; preloaded from Postgres, updated in place as
/// blocks are materialized in order.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub orders: HashMap<u64, OrderInfo>,
    pub outbounds: HashMap<u64, OutboundInfo>,
    pub trades: HashMap<u64, TradeInfo>,
    pub offers: HashMap<u64, OfferInfo>,
    /// (owner, asset, amount) → deposit key, for held deposits awaiting release.
    pub held: HashMap<(String, String, Amount), String>,
    pub params: HashMap<String, Amount>,
    /// pair → (base asset, quote asset)
    pub markets: HashMap<String, (String, String)>,
}

/// Ids a batch of blocks references, so the DB layer can preload `Ctx`.
#[derive(Clone, Debug, Default)]
pub struct CtxRefs {
    pub orders: HashSet<u64>,
    pub outbounds: HashSet<u64>,
    pub trades: HashSet<u64>,
    pub offers: HashSet<u64>,
    pub params: HashSet<String>,
    pub held: bool,
}

pub fn collect_refs(blocks: &[BlockData]) -> CtxRefs {
    let mut r = CtxRefs::default();
    for b in blocks {
        let all = b
            .receipts
            .iter()
            .flat_map(|x| x.events.iter())
            .chain(b.events.iter());
        for ev in all {
            let (t, f) = split_event(ev);
            match t.as_str() {
                "OrderFilled" => {
                    field_u64(&f, "order_id").map(|i| r.orders.insert(i));
                    field_u64(&f, "maker_order_id").map(|i| r.orders.insert(i));
                }
                "OrderCancelled" | "OrderAccepted" => {
                    field_u64(&f, "order_id").map(|i| r.orders.insert(i));
                }
                "OutboundBatched"
                | "OutboundConfirmed"
                | "OutboundFailed"
                | "LightningPayoutAssigned"
                | "LightningPayoutSettled"
                | "LightningPayoutFailed" => {
                    field_u64(&f, "outbound_id").map(|i| r.outbounds.insert(i));
                }
                "TradeStarted" => {
                    field_u64(&f, "offer_id").map(|i| r.offers.insert(i));
                }
                "TradePaid" | "TradeReleased" | "TradeCancelled" | "DisputeOpened"
                | "DisputeRuled" => {
                    field_u64(&f, "trade_id").map(|i| r.trades.insert(i));
                }
                "OfferUpdated" | "OfferClosed" => {
                    field_u64(&f, "offer_id").map(|i| r.offers.insert(i));
                }
                "DepositReleased" => r.held = true,
                "ParamChanged" => {
                    field_str(&f, "key").map(|k| r.params.insert(k));
                }
                _ => {}
            }
        }
    }
    r
}

// ---------------- rows ----------------

#[derive(Clone, Debug, Default)]
pub struct BlockRow {
    pub height: i64,
    pub timestamp: i64,
    pub timestamp_exact: bool,
    pub state_hash: Option<String>,
    pub tx_count: i32,
    pub ok_count: i32,
    pub event_count: i32,
    pub proposer: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TxRow {
    pub height: i64,
    pub index: i32,
    pub tx_id: String,
    pub timestamp: i64,
    pub signer: String,
    pub nonce: Option<i64>,
    pub module: String,
    pub kind: String,
    pub action: Option<Value>,
    pub ok: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub event_count: i32,
}

#[derive(Clone, Debug, Default)]
pub struct EventRow {
    pub height: i64,
    pub tx_index: i32,
    pub event_index: i32,
    pub tx_id: Option<String>,
    pub timestamp: i64,
    pub kind: String,
    pub data: Value,
    pub addresses: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransferRow {
    pub height: i64,
    pub tx_index: i32,
    pub event_index: i32,
    pub leg: i16,
    pub tx_id: Option<String>,
    pub timestamp: i64,
    pub asset: String,
    pub amount: Amount,
    pub from: Option<String>,
    pub to: Option<String>,
    pub kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FillRow {
    pub height: i64,
    pub tx_index: i32,
    pub event_index: i32,
    pub tx_id: Option<String>,
    pub timestamp: i64,
    pub pair: String,
    pub price: Amount,
    pub quantity: Amount,
    pub quote: Amount,
    pub fee: Amount,
    pub fee_asset: Option<String>,
    pub taker: Option<String>,
    pub taker_side: Option<String>,
    pub taker_order_id: i64,
    pub maker_order_id: Option<i64>,
    pub maker: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Candle {
    pub o: Amount,
    pub h: Amount,
    pub l: Amount,
    pub c: Amount,
    pub v: Amount,
    pub qv: Amount,
    pub n: i32,
}

impl Candle {
    pub fn add(&mut self, price: Amount, qty: Amount, quote: Amount) {
        if self.n == 0 {
            *self = Candle {
                o: price,
                h: price,
                l: price,
                c: price,
                v: qty,
                qv: quote,
                n: 1,
            };
        } else {
            self.h = self.h.max(price);
            self.l = self.l.min(price);
            self.c = price;
            self.v = self.v.saturating_add(qty);
            self.qv = self.qv.saturating_add(quote);
            self.n += 1;
        }
    }
    /// Merge a later candle into this one (same bucket).
    pub fn merge(&mut self, later: &Candle) {
        if later.n == 0 {
            return;
        }
        if self.n == 0 {
            *self = later.clone();
            return;
        }
        self.h = self.h.max(later.h);
        self.l = self.l.min(later.l);
        self.c = later.c;
        self.v = self.v.saturating_add(later.v);
        self.qv = self.qv.saturating_add(later.qv);
        self.n += later.n;
    }
}

pub const MINUTE_MS: i64 = 60_000;

pub fn bucket_1m(timestamp_ms: i64) -> i64 {
    timestamp_ms - timestamp_ms.rem_euclid(MINUTE_MS)
}

#[derive(Clone, Debug, Default)]
pub struct AccountDelta {
    pub first_seen: i64,
    pub last_seen: i64,
    pub tx_count: i64,
}

/// Patches for state-like tables. `None` = leave unchanged.
#[derive(Clone, Debug, Default)]
pub struct OrderPatch {
    pub id: i64,
    pub owner: Option<String>,
    pub pair: Option<String>,
    pub side: Option<String>,
    pub order_type: Option<String>,
    pub price: Option<Amount>,
    pub quantity: Option<Amount>,
    pub quote_budget: Option<Amount>,
    pub client_id: Option<i64>,
    pub resting: Option<Amount>,
    pub filled_delta: Amount,
    pub filled_quote_delta: Amount,
    pub released: Option<Amount>,
    pub status: Option<String>,
    pub created_height: Option<i64>,
    pub updated_height: i64,
    pub tx_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct OfferPatch {
    pub id: i64,
    pub owner: Option<String>,
    pub spec: Option<Value>,
    pub status: Option<String>,
    pub created_height: Option<i64>,
    pub updated_height: i64,
    pub tx_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TradePatch {
    pub id: i64,
    pub offer_id: Option<i64>,
    pub buyer: Option<String>,
    pub seller: Option<String>,
    pub asset: Option<String>,
    pub amount: Option<Amount>,
    pub fee: Option<Amount>,
    pub fiat_amount: Option<Amount>,
    pub fiat_currency: Option<String>,
    pub status: Option<String>,
    pub started_height: Option<i64>,
    pub started_at: Option<i64>,
    pub deadline: Option<i64>,
    pub paid_at: Option<i64>,
    pub closed_height: Option<i64>,
    pub dispute: Option<Value>,
    pub history: Option<Value>,
    pub updated_height: i64,
    pub tx_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DepositPatch {
    pub key: String,
    pub chain: Option<String>,
    pub asset: Option<String>,
    pub owner: Option<String>,
    pub amount: Option<Amount>,
    pub status: String,
    pub tx_hash: Option<String>,
    pub external_index: Option<i32>,
    pub deposit_index: Option<i64>,
    pub external_height: Option<i64>,
    pub votes: Option<i32>,
    pub height: i64,
    pub tx_id: Option<String>,
    pub release_height: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct OutboundPatch {
    pub id: i64,
    pub owner: Option<String>,
    pub asset: Option<String>,
    pub chain: Option<String>,
    pub to: Option<String>,
    pub amount: Option<Amount>,
    pub status: Option<String>,
    pub batch_id: Option<i64>,
    pub tx_hash: Option<String>,
    pub refunded: Option<Amount>,
    pub created_height: Option<i64>,
    pub confirmed_height: Option<i64>,
    pub updated_height: i64,
    pub tx_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ProposalPatch {
    pub id: i64,
    pub proposer: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub kind: Option<Value>,
    pub status: Option<String>,
    pub submit_height: Option<i64>,
    pub tally: Option<(String, Amount)>,
    pub executed_ok: Option<bool>,
    pub updated_height: i64,
    pub tx_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct VoteRow {
    pub proposal_id: i64,
    pub voter: String,
    pub choice: Option<String>,
    pub weight: Amount,
    pub height: i64,
    pub tx_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ParamRow {
    pub height: i64,
    pub tx_index: i32,
    pub event_index: i32,
    pub timestamp: i64,
    pub key: String,
    pub from: Option<Amount>,
    pub to: Amount,
    pub tx_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct EpochRow {
    pub epoch: i64,
    pub start_height: i64,
    pub validator_count: i32,
}

#[derive(Clone, Debug)]
pub enum Patch {
    Order(OrderPatch),
    Offer(OfferPatch),
    Trade(TradePatch),
    Deposit(DepositPatch),
    Outbound(OutboundPatch),
    Proposal(ProposalPatch),
    Vote(VoteRow),
    Param(ParamRow),
    Epoch(EpochRow),
}

/// Everything one block turns into.
#[derive(Clone, Debug, Default)]
pub struct Materialized {
    pub block: BlockRow,
    pub txs: Vec<TxRow>,
    pub events: Vec<EventRow>,
    pub transfers: Vec<TransferRow>,
    pub fills: Vec<FillRow>,
    pub candles: BTreeMap<(String, i64), Candle>,
    pub accounts: BTreeMap<String, AccountDelta>,
    pub patches: Vec<Patch>,
    /// Ids touched, for post-batch enrichment from the node's snapshots.
    pub touched: Touched,
}

#[derive(Clone, Debug, Default)]
pub struct Touched {
    pub orders: HashSet<u64>,
    pub offers: HashSet<u64>,
    pub trades: HashSet<u64>,
    pub proposals: HashSet<u64>,
    pub outbounds: HashSet<u64>,
    pub epoch_advanced: bool,
    pub asset_registered: bool,
}

impl Touched {
    pub fn merge(&mut self, o: &Touched) {
        self.orders.extend(o.orders.iter().copied());
        self.offers.extend(o.offers.iter().copied());
        self.trades.extend(o.trades.iter().copied());
        self.proposals.extend(o.proposals.iter().copied());
        self.outbounds.extend(o.outbounds.iter().copied());
        self.epoch_advanced |= o.epoch_advanced;
        self.asset_registered |= o.asset_registered;
    }
}

// ---------------- action decoding ----------------

/// `(kind, module)` of a decoded action. Kind is the `Action` variant name;
/// module follows `keel_actions::Action::module`, with a static fallback
/// when the JSON no longer deserializes into this build's `Action`.
pub fn action_kind(action: &Value) -> (String, String) {
    let kind = match action {
        Value::Object(m) if m.len() == 1 => m.keys().next().cloned().unwrap_or_default(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    };
    if kind.is_empty() {
        return ("unknown".into(), "unknown".into());
    }
    if let Ok(a) = serde_json::from_value::<keel_actions::Action>(action.clone()) {
        return (kind, a.module().to_string());
    }
    (kind.clone(), module_of_kind(&kind).to_string())
}

pub fn module_of_kind(kind: &str) -> &'static str {
    match kind {
        "Transfer" => "tokens",
        "PlaceOrder" | "CancelOrder" | "HouseQuote" => "markets",
        "BuyBudget" | "LockBudget" | "UnlockBudget" => "budgets",
        "AuthorizeSessionKey" | "RevokeSessionKey" => "sessions",
        "RegisterLightningNode"
        | "ObserveLightningDeposit"
        | "ObserveLightningPayout"
        | "FundLightningPool"
        | "AnnounceLightningSweep" => "lightning",
        "CreateOffer" | "UpdateOffer" | "PauseOffer" | "CloseOffer" | "StartTrade" | "MarkPaid"
        | "ReleaseTrade" | "CancelTrade" => "market_p2p",
        "OpenDispute" | "SubmitEvidence" | "RuleDispute" => "disputes",
        "RequestDepositAddress"
        | "ObserveDeposit"
        | "ObserveOutbound"
        | "ReportNetworkFee"
        | "Withdraw"
        | "RegisterVault" => "vaults",
        "MintStable" | "BurnStable" => "stable",
        "Bond" | "Unbond" | "Delegate" | "Undelegate" | "ClaimRewards" => "staking",
        "Propose" | "Vote" | "ExecuteProposal" | "SetParam" => "gov",
        "Attest" => "attest",
        _ => "unknown",
    }
}

/// Without the actions route the kind is inferred from what the receipt
/// emitted (failed actions emit nothing and stay `unknown`).
pub fn infer_kind_from_events(events: &[Value]) -> (String, String) {
    let mut types: Vec<String> = events.iter().map(|e| split_event(e).0).collect();
    types.dedup();
    let kind = if types
        .iter()
        .any(|t| t == "OrderAccepted" || t == "OrderRejected" || t == "OrderFilled")
    {
        "PlaceOrder"
    } else {
        match types.first().map(String::as_str) {
            Some("OrderCancelled") => "CancelOrder",
            Some("Transferred") => "Transfer",
            Some("BudgetPurchased") => "BuyBudget",
            Some("OfferCreated") => "CreateOffer",
            Some("OfferUpdated") => "UpdateOffer",
            Some("OfferClosed") => "CloseOffer",
            Some("TradeStarted") => "StartTrade",
            Some("TradePaid") => "MarkPaid",
            Some("TradeReleased") => "ReleaseTrade",
            Some("TradeCancelled") => "CancelTrade",
            Some("DisputeOpened") => "OpenDispute",
            Some("DisputeRuled") => "RuleDispute",
            Some("DepositAddressAssigned") => "RequestDepositAddress",
            Some("DepositObserved") | Some("DepositCredited") | Some("DepositHeld") => {
                "ObserveDeposit"
            }
            Some("OutboundConfirmed") | Some("OutboundFailed") => "ObserveOutbound",
            Some("LightningDepositCredited") => "ObserveLightningDeposit",
            Some("LightningPayoutSettled") | Some("LightningPayoutFailed") => {
                "ObserveLightningPayout"
            }
            Some("NetworkFeeReported") => "ReportNetworkFee",
            Some("WithdrawalQueued") => "Withdraw",
            Some("VaultRegistered") => "RegisterVault",
            Some("StableMinted") => "MintStable",
            Some("StableBurned") => "BurnStable",
            Some("Bonded") => "Bond",
            Some("Unbonded") => "Unbond",
            Some("Delegated") => "Delegate",
            Some("Undelegated") => "Undelegate",
            Some("RewardsClaimed") => "ClaimRewards",
            Some("ProposalCreated") => "Propose",
            Some("Voted") => "Vote",
            Some("ProposalExecuted") => "ExecuteProposal",
            Some("ParamChanged") => "SetParam",
            Some("ParamAdminChanged") => "ExecuteProposal",
            Some("Attested") => "Attest",
            _ => "unknown",
        }
    };
    (kind.to_string(), module_of_kind(kind).to_string())
}

fn inner_action(action: &Value) -> Value {
    match action {
        Value::Object(m) if m.len() == 1 => m.values().next().cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

// ---------------- materialization ----------------

/// `prev_timestamp` is the previous block's time, used when the node served
/// no timestamp for this height (empty block via the receipts route).
pub fn materialize(b: &BlockData, ctx: &mut Ctx, prev_timestamp: i64) -> Materialized {
    let height = b.height as i64;
    let (timestamp, exact) = match b.best_timestamp() {
        Some(t) => (t as i64, true),
        None => (prev_timestamp, false),
    };
    let mut m = Materialized {
        block: BlockRow {
            height,
            timestamp,
            timestamp_exact: exact,
            state_hash: b.state_hash.clone(),
            tx_count: b.receipts.len() as i32,
            ok_count: b.receipts.iter().filter(|r| r.ok).count() as i32,
            event_count: (b.receipts.iter().map(|r| r.events.len()).sum::<usize>() + b.events.len())
                as i32,
            proposer: b.proposer.clone(),
        },
        ..Default::default()
    };
    let actions: HashMap<&str, &ActionRow> = b
        .actions
        .iter()
        .flatten()
        .map(|a| (a.tx_id.as_str(), a))
        .collect();
    let actions_by_index: HashMap<u32, &ActionRow> =
        b.actions.iter().flatten().map(|a| (a.index, a)).collect();

    for r in &b.receipts {
        let action = actions
            .get(r.tx_id.as_str())
            .or_else(|| {
                actions_by_index
                    .get(&r.index)
                    .filter(|a| a.tx_id.is_empty() || a.tx_id == r.tx_id)
            })
            .copied();
        let (kind, module) = match action {
            Some(a) => action_kind(&a.action),
            None => infer_kind_from_events(&r.events),
        };
        m.txs.push(TxRow {
            height,
            index: r.index as i32,
            tx_id: r.tx_id.clone(),
            timestamp,
            signer: r.signer.clone(),
            nonce: action.and_then(|a| a.nonce).map(|n| n as i64),
            module,
            kind: kind.clone(),
            action: action.map(|a| a.action.clone()),
            ok: r.ok,
            error_code: r.error.as_ref().map(|e| e.0.clone()),
            error_message: r.error.as_ref().map(|e| e.1.clone()),
            event_count: r.events.len() as i32,
        });
        touch_account(&mut m.accounts, &r.signer, height, 1);
        let inner = action
            .map(|a| inner_action(&a.action))
            .unwrap_or(Value::Null);
        let mut scope = ReceiptScope {
            signer: r.signer.clone(),
            tx_id: Some(r.tx_id.clone()),
            tx_index: r.index as i32,
            kind,
            action: inner,
            last_deposit_key: None,
        };
        for (i, ev) in r.events.iter().enumerate() {
            apply_event(&mut m, ctx, &mut scope, ev, i as i32, height, timestamp);
        }
    }
    let mut scope = ReceiptScope {
        signer: String::new(),
        tx_id: None,
        tx_index: -1,
        kind: String::new(),
        action: Value::Null,
        last_deposit_key: None,
    };
    for (i, ev) in b.events.iter().enumerate() {
        apply_event(&mut m, ctx, &mut scope, ev, i as i32, height, timestamp);
    }
    m
}

struct ReceiptScope {
    signer: String,
    tx_id: Option<String>,
    tx_index: i32,
    kind: String,
    /// Inner payload of the decoded action (`Null` when unknown).
    action: Value,
    last_deposit_key: Option<String>,
}

fn touch_account(acc: &mut BTreeMap<String, AccountDelta>, addr: &str, height: i64, txs: i64) {
    if addr.is_empty() {
        return;
    }
    let d = acc.entry(addr.to_string()).or_insert(AccountDelta {
        first_seen: height,
        last_seen: height,
        tx_count: 0,
    });
    d.first_seen = d.first_seen.min(height);
    d.last_seen = d.last_seen.max(height);
    d.tx_count += txs;
}

fn secs(ts_ms: i64) -> i64 {
    ts_ms / 1000
}

fn lower(s: Option<String>) -> Option<String> {
    s.map(|x| x.to_ascii_lowercase())
}

/// `{"buy": ...}` / `"buy"`: serde of `Side` is lowercase.
fn side_of(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_ascii_lowercase())
}

#[allow(clippy::too_many_lines)]
fn apply_event(
    m: &mut Materialized,
    ctx: &mut Ctx,
    scope: &mut ReceiptScope,
    ev: &Value,
    event_index: i32,
    height: i64,
    timestamp: i64,
) {
    let (t, f) = split_event(ev);
    let addrs = addresses_in(ev);
    for a in &addrs {
        touch_account(&mut m.accounts, a, height, 0);
    }
    m.events.push(EventRow {
        height,
        tx_index: scope.tx_index,
        event_index,
        tx_id: scope.tx_id.clone(),
        timestamp,
        kind: t.clone(),
        data: f.clone(),
        addresses: addrs,
    });
    let tx_id = scope.tx_id.clone();
    let mut transfer = |leg: i16,
                        asset: &str,
                        amount: Amount,
                        from: Option<String>,
                        to: Option<String>,
                        kind: &str| {
        m.transfers.push(TransferRow {
            height,
            tx_index: scope.tx_index,
            event_index,
            leg,
            tx_id: tx_id.clone(),
            timestamp,
            asset: asset.to_string(),
            amount,
            from,
            to,
            kind: kind.to_string(),
        });
    };
    match t.as_str() {
        "Transferred" => {
            if let (Some(asset), Some(amount)) =
                (field_str(&f, "asset"), field_amount(&f, "amount"))
            {
                transfer(
                    0,
                    &asset,
                    amount,
                    field_addr(&f, "from"),
                    field_addr(&f, "to"),
                    "transfer",
                );
            }
        }
        "DepositCredited" | "DepositReleased" => {
            let owner = field_addr(&f, "owner");
            let asset = field_str(&f, "asset").unwrap_or_default();
            let amount = field_amount(&f, "amount").unwrap_or(0);
            transfer(0, &asset, amount, None, owner.clone(), "deposit");
            let (key, status) = if t == "DepositReleased" {
                let k = owner
                    .clone()
                    .zip(Some(asset.clone()))
                    .and_then(|(o, a)| ctx.held.remove(&(o, a, amount)));
                (
                    k.unwrap_or_else(|| {
                        format!("release:{height}:{}:{event_index}", scope.tx_index)
                    }),
                    "released",
                )
            } else {
                (
                    scope.last_deposit_key.clone().unwrap_or_else(|| {
                        format!("credit:{height}:{}:{event_index}", scope.tx_index)
                    }),
                    "credited",
                )
            };
            m.patches.push(Patch::Deposit(DepositPatch {
                key,
                chain: field_str(&f, "asset")
                    .and_then(|a| crate::types::asset_chain(&a).map(str::to_string)),
                asset: Some(asset),
                owner,
                amount: Some(amount),
                status: status.into(),
                height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "DepositHeld" => {
            let owner = field_addr(&f, "owner").unwrap_or_default();
            let asset = field_str(&f, "asset").unwrap_or_default();
            let amount = field_amount(&f, "amount").unwrap_or(0);
            let key = scope
                .last_deposit_key
                .clone()
                .unwrap_or_else(|| format!("held:{height}:{}:{event_index}", scope.tx_index));
            ctx.held
                .insert((owner.clone(), asset.clone(), amount), key.clone());
            m.patches.push(Patch::Deposit(DepositPatch {
                key,
                chain: crate::types::asset_chain(&asset).map(str::to_string),
                asset: Some(asset),
                owner: Some(owner),
                amount: Some(amount),
                status: "held".into(),
                height,
                tx_id: scope.tx_id.clone(),
                release_height: field_u64(&f, "release_height").map(|x| x as i64),
                ..Default::default()
            }));
        }
        "DepositObserved" => {
            let chain = field_str(&f, "chain").unwrap_or_default();
            let tx_hash = field_str(&f, "tx_hash")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let index = field_u64(&scope.action, "index").map(|x| x as i32);
            let key = match index {
                Some(i) => format!("{chain}:{tx_hash}:{i}"),
                None => format!("{chain}:{tx_hash}"),
            };
            scope.last_deposit_key = Some(key.clone());
            m.patches.push(Patch::Deposit(DepositPatch {
                key,
                chain: Some(chain),
                asset: field_str(&scope.action, "asset"),
                amount: field_amount(&scope.action, "amount"),
                status: "pending".into(),
                tx_hash: Some(tx_hash),
                external_index: index,
                deposit_index: field_u64(&scope.action, "deposit_index").map(|x| x as i64),
                external_height: field_u64(&scope.action, "external_height").map(|x| x as i64),
                votes: field_u64(&f, "votes").map(|x| x as i32),
                height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "OrderAccepted" => {
            let id = field_u64(&f, "order_id").unwrap_or(0);
            let owner = field_addr(&f, "owner").unwrap_or_else(|| scope.signer.clone());
            let pair = field_str(&f, "pair").unwrap_or_default();
            let resting = field_amount(&f, "resting").unwrap_or(0);
            let from_action = scope.kind == "PlaceOrder";
            let side = if from_action {
                scope.action.get("side").and_then(side_of)
            } else {
                None
            };
            ctx.orders.insert(
                id,
                OrderInfo {
                    owner: owner.clone(),
                    pair: pair.clone(),
                    side: side.clone(),
                },
            );
            m.touched.orders.insert(id);
            m.patches.push(Patch::Order(OrderPatch {
                id: id as i64,
                owner: Some(owner),
                pair: Some(pair),
                side,
                order_type: if from_action {
                    scope.action.get("order_type").and_then(side_of)
                } else {
                    None
                },
                price: if from_action {
                    field_amount(&scope.action, "price")
                } else {
                    None
                },
                quantity: if from_action {
                    field_amount(&scope.action, "quantity")
                } else {
                    None
                },
                quote_budget: if from_action {
                    field_amount(&scope.action, "quote_budget")
                } else {
                    None
                },
                client_id: if from_action {
                    field_u64(&scope.action, "client_id").map(|x| x as i64)
                } else {
                    None
                },
                resting: Some(resting),
                status: Some(if resting == 0 {
                    "filled".into()
                } else {
                    "open".into()
                }),
                created_height: Some(height),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "OrderFilled" => {
            let order_id = field_u64(&f, "order_id").unwrap_or(0);
            let maker_order_id = field_u64(&f, "maker_order_id");
            let pair = field_str(&f, "pair").unwrap_or_default();
            let price = field_amount(&f, "price").unwrap_or(0);
            let quantity = field_amount(&f, "quantity").unwrap_or(0);
            let quote = field_amount(&f, "quote").unwrap_or(0);
            let fee = field_amount(&f, "fee").unwrap_or(0);
            let taker_info = ctx.orders.get(&order_id).cloned();
            let taker = taker_info
                .as_ref()
                .map(|o| o.owner.clone())
                .filter(|o| !o.is_empty())
                .or_else(|| (!scope.signer.is_empty()).then(|| scope.signer.clone()));
            let taker_side = taker_info
                .as_ref()
                .and_then(|o| o.side.clone())
                .or_else(|| {
                    if scope.kind == "PlaceOrder" {
                        scope.action.get("side").and_then(side_of)
                    } else {
                        None
                    }
                });
            let maker = match maker_order_id {
                Some(mid) => ctx.orders.get(&mid).map(|o| o.owner.clone()),
                None => Some(SYSTEM_ADDR.to_string()),
            };
            let (base, quote_asset) = ctx.markets.get(&pair).cloned().unwrap_or_else(|| {
                let (b, q) = pair.split_once('-').unwrap_or((pair.as_str(), ""));
                (b.to_string(), q.to_string())
            });
            let fee_asset = match taker_side.as_deref() {
                Some("buy") => Some(base.clone()),
                Some("sell") => Some(quote_asset.clone()),
                _ => None,
            };
            match taker_side.as_deref() {
                Some("buy") => {
                    transfer(
                        0,
                        &base,
                        quantity,
                        maker.clone(),
                        taker.clone(),
                        "fill_base",
                    );
                    transfer(
                        1,
                        &quote_asset,
                        quote,
                        taker.clone(),
                        maker.clone(),
                        "fill_quote",
                    );
                }
                Some("sell") => {
                    transfer(
                        0,
                        &base,
                        quantity,
                        taker.clone(),
                        maker.clone(),
                        "fill_base",
                    );
                    transfer(
                        1,
                        &quote_asset,
                        quote,
                        maker.clone(),
                        taker.clone(),
                        "fill_quote",
                    );
                }
                _ => {}
            }
            if fee > 0 {
                if let Some(fa) = &fee_asset {
                    transfer(
                        2,
                        fa,
                        fee,
                        taker.clone(),
                        Some(SYSTEM_ADDR.into()),
                        "fill_fee",
                    );
                }
            }
            m.fills.push(FillRow {
                height,
                tx_index: scope.tx_index,
                event_index,
                tx_id: scope.tx_id.clone(),
                timestamp,
                pair: pair.clone(),
                price,
                quantity,
                quote,
                fee,
                fee_asset,
                taker,
                taker_side,
                taker_order_id: order_id as i64,
                maker_order_id: maker_order_id.map(|x| x as i64),
                maker,
            });
            m.candles
                .entry((pair, bucket_1m(timestamp)))
                .or_default()
                .add(price, quantity, quote);
            m.touched.orders.insert(order_id);
            m.patches.push(Patch::Order(OrderPatch {
                id: order_id as i64,
                filled_delta: quantity,
                filled_quote_delta: quote,
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
            if let Some(mid) = maker_order_id {
                m.touched.orders.insert(mid);
                m.patches.push(Patch::Order(OrderPatch {
                    id: mid as i64,
                    filled_delta: quantity,
                    filled_quote_delta: quote,
                    updated_height: height,
                    ..Default::default()
                }));
            }
        }
        "OrderCancelled" => {
            let id = field_u64(&f, "order_id").unwrap_or(0);
            m.touched.orders.insert(id);
            m.patches.push(Patch::Order(OrderPatch {
                id: id as i64,
                released: field_amount(&f, "released"),
                status: Some("cancelled".into()),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "OfferCreated" => {
            let id = field_u64(&f, "offer_id").unwrap_or(0);
            let owner = field_addr(&f, "owner").unwrap_or_else(|| scope.signer.clone());
            let spec = (scope.kind == "CreateOffer" && scope.action.is_object())
                .then(|| scope.action.clone());
            ctx.offers.insert(
                id,
                OfferInfo {
                    owner: owner.clone(),
                    asset: field_str(&scope.action, "asset").unwrap_or_default(),
                    fiat_currency: field_str(&scope.action, "fiat_currency"),
                },
            );
            m.touched.offers.insert(id);
            m.patches.push(Patch::Offer(OfferPatch {
                id: id as i64,
                owner: Some(owner),
                spec,
                status: Some("open".into()),
                created_height: Some(height),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
            }));
        }
        "OfferUpdated" => {
            let id = field_u64(&f, "offer_id").unwrap_or(0);
            let (spec, status) = match scope.kind.as_str() {
                "UpdateOffer" => (scope.action.get("spec").cloned(), None),
                "PauseOffer" => (
                    None,
                    Some(
                        if scope
                            .action
                            .get("paused")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                        {
                            "paused"
                        } else {
                            "open"
                        }
                        .to_string(),
                    ),
                ),
                _ => (None, None),
            };
            m.touched.offers.insert(id);
            m.patches.push(Patch::Offer(OfferPatch {
                id: id as i64,
                spec,
                status,
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "OfferClosed" => {
            let id = field_u64(&f, "offer_id").unwrap_or(0);
            m.touched.offers.insert(id);
            m.patches.push(Patch::Offer(OfferPatch {
                id: id as i64,
                status: Some("closed".into()),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "TradeStarted" => {
            let id = field_u64(&f, "trade_id").unwrap_or(0);
            let offer_id = field_u64(&f, "offer_id").unwrap_or(0);
            let buyer = field_addr(&f, "buyer").unwrap_or_default();
            let seller = field_addr(&f, "seller").unwrap_or_default();
            let amount = field_amount(&f, "amount").unwrap_or(0);
            let offer = ctx.offers.get(&offer_id).cloned();
            let asset = offer
                .as_ref()
                .map(|o| o.asset.clone())
                .filter(|a| !a.is_empty());
            ctx.trades.insert(
                id,
                TradeInfo {
                    buyer: buyer.clone(),
                    seller: seller.clone(),
                    asset: asset.clone().unwrap_or_default(),
                    amount,
                },
            );
            m.touched.trades.insert(id);
            m.patches.push(Patch::Trade(TradePatch {
                id: id as i64,
                offer_id: Some(offer_id as i64),
                buyer: Some(buyer),
                seller: Some(seller),
                asset,
                amount: Some(amount),
                fiat_amount: field_amount(&scope.action, "fiat_amount"),
                fiat_currency: offer.and_then(|o| o.fiat_currency),
                status: Some("funded".into()),
                started_height: Some(height),
                started_at: Some(secs(timestamp)),
                history: Some(json!({"status": "funded", "height": height, "timestamp": timestamp, "tx_id": scope.tx_id})),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "TradePaid" | "TradeReleased" | "TradeCancelled" | "DisputeOpened" | "DisputeRuled" => {
            let id = field_u64(&f, "trade_id").unwrap_or(0);
            let status = match t.as_str() {
                "TradePaid" => "paid",
                "TradeReleased" => "released",
                "TradeCancelled" => "cancelled",
                "DisputeOpened" => "disputed",
                _ => "ruled",
            };
            let mut p = TradePatch {
                id: id as i64,
                status: Some(status.into()),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            };
            let mut hist = json!({"status": status, "height": height, "timestamp": timestamp, "tx_id": scope.tx_id});
            let info = ctx.trades.get(&id).cloned();
            match t.as_str() {
                "TradePaid" => p.paid_at = Some(secs(timestamp)),
                "TradeReleased" => {
                    p.fee = field_amount(&f, "fee");
                    p.closed_height = Some(height);
                    if let Some(ti) = &info {
                        let to_buyer = field_amount(&f, "to_buyer").unwrap_or(ti.amount);
                        transfer(
                            0,
                            &ti.asset,
                            to_buyer,
                            Some(ti.seller.clone()),
                            Some(ti.buyer.clone()),
                            "trade_release",
                        );
                        if let Some(fee) = field_amount(&f, "fee").filter(|x| *x > 0) {
                            transfer(
                                1,
                                &ti.asset,
                                fee,
                                Some(ti.seller.clone()),
                                Some(SYSTEM_ADDR.into()),
                                "trade_fee",
                            );
                        }
                    }
                }
                "TradeCancelled" => {
                    p.closed_height = Some(height);
                    hist["by"] = json!(field_addr(&f, "by"));
                }
                "DisputeOpened" => {
                    p.dispute = Some(
                        json!({"opened_by": field_addr(&f, "by"), "opened_height": height, "opened_at": secs(timestamp), "ruling": null}),
                    );
                    hist["by"] = json!(field_addr(&f, "by"));
                }
                _ => {
                    let buyer_amount = field_amount(&f, "buyer_amount").unwrap_or(0);
                    let seller_amount = field_amount(&f, "seller_amount").unwrap_or(0);
                    p.closed_height = Some(height);
                    p.dispute = Some(
                        json!({"ruling": {"buyer_amount": buyer_amount.to_string(), "seller_amount": seller_amount.to_string()}, "ruled_height": height, "ruled_by": scope.signer}),
                    );
                    if let Some(ti) = &info {
                        if buyer_amount > 0 {
                            transfer(
                                0,
                                &ti.asset,
                                buyer_amount,
                                Some(ti.seller.clone()),
                                Some(ti.buyer.clone()),
                                "dispute_ruling",
                            );
                        }
                    }
                }
            }
            p.history = Some(hist);
            m.touched.trades.insert(id);
            m.patches.push(Patch::Trade(p));
        }
        "WithdrawalQueued" => {
            let id = field_u64(&f, "outbound_id").unwrap_or(0);
            let owner = field_addr(&f, "owner").unwrap_or_else(|| scope.signer.clone());
            let asset = field_str(&f, "asset").unwrap_or_default();
            let amount = field_amount(&f, "amount").unwrap_or(0);
            ctx.outbounds.insert(
                id,
                OutboundInfo {
                    owner: owner.clone(),
                    asset: asset.clone(),
                    amount,
                },
            );
            m.touched.outbounds.insert(id);
            m.patches.push(Patch::Outbound(OutboundPatch {
                id: id as i64,
                owner: Some(owner),
                chain: crate::types::asset_chain(&asset).map(str::to_string),
                asset: Some(asset),
                to: field_str(&f, "to"),
                amount: Some(amount),
                status: Some("queued".into()),
                created_height: Some(height),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "OutboundBatched" => {
            let id = field_u64(&f, "outbound_id").unwrap_or(0);
            m.touched.outbounds.insert(id);
            m.patches.push(Patch::Outbound(OutboundPatch {
                id: id as i64,
                chain: field_str(&f, "chain"),
                status: Some("batched".into()),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "OutboundConfirmed" => {
            let id = field_u64(&f, "outbound_id").unwrap_or(0);
            if let Some(o) = ctx.outbounds.get(&id) {
                transfer(
                    0,
                    &o.asset,
                    o.amount,
                    Some(o.owner.clone()),
                    None,
                    "withdrawal",
                );
            }
            m.touched.outbounds.insert(id);
            m.patches.push(Patch::Outbound(OutboundPatch {
                id: id as i64,
                status: Some("confirmed".into()),
                tx_hash: lower(field_str(&f, "tx_hash")),
                confirmed_height: Some(height),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        // Lightning (2026-09-10): a settled invoice credits the owner
        // like a deposit and a paid payout closes its outbound like a
        // confirmed one, but the events differ so the wallet history can
        // label them "Lightning" and the outbound keeps the payment hash.
        "LightningDepositCredited" => {
            let owner = field_addr(&f, "owner");
            let amount = field_amount(&f, "amount").unwrap_or(0);
            transfer(0, "BTC.BTC", amount, None, owner, "lightning_deposit");
        }
        "LightningPayoutSettled" => {
            let id = field_u64(&f, "outbound_id").unwrap_or(0);
            if let Some(o) = ctx.outbounds.get(&id) {
                transfer(
                    0,
                    &o.asset,
                    o.amount,
                    Some(o.owner.clone()),
                    None,
                    "lightning_payout",
                );
            }
            m.touched.outbounds.insert(id);
            m.patches.push(Patch::Outbound(OutboundPatch {
                id: id as i64,
                status: Some("confirmed".into()),
                confirmed_height: Some(height),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "LightningPayoutFailed" => {
            let id = field_u64(&f, "outbound_id").unwrap_or(0);
            m.touched.outbounds.insert(id);
            m.patches.push(Patch::Outbound(OutboundPatch {
                id: id as i64,
                status: Some("failed".into()),
                refunded: field_amount(&f, "refunded"),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "OutboundFailed" => {
            let id = field_u64(&f, "outbound_id").unwrap_or(0);
            m.touched.outbounds.insert(id);
            m.patches.push(Patch::Outbound(OutboundPatch {
                id: id as i64,
                status: Some("failed".into()),
                refunded: field_amount(&f, "refunded"),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "ProposalCreated" => {
            let id = field_u64(&f, "proposal_id").unwrap_or(0);
            let from_action = scope.kind == "Propose";
            m.touched.proposals.insert(id);
            m.patches.push(Patch::Proposal(ProposalPatch {
                id: id as i64,
                proposer: field_addr(&f, "proposer").or_else(|| Some(scope.signer.clone())),
                title: if from_action {
                    field_str(&scope.action, "title")
                } else {
                    None
                },
                description: if from_action {
                    field_str(&scope.action, "description")
                } else {
                    None
                },
                kind: if from_action {
                    scope.action.get("kind").cloned()
                } else {
                    None
                },
                status: Some("voting".into()),
                submit_height: Some(height),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "Voted" => {
            let id = field_u64(&f, "proposal_id").unwrap_or(0);
            let voter = field_addr(&f, "voter").unwrap_or_else(|| scope.signer.clone());
            let weight = field_amount(&f, "weight").unwrap_or(0);
            let choice = (scope.kind == "Vote")
                .then(|| {
                    scope
                        .action
                        .get("choice")
                        .and_then(|c| c.as_str().map(|s| s.to_ascii_lowercase()))
                })
                .flatten();
            m.touched.proposals.insert(id);
            m.patches.push(Patch::Vote(VoteRow {
                proposal_id: id as i64,
                voter,
                choice: choice.clone(),
                weight,
                height,
                tx_id: scope.tx_id.clone(),
            }));
            m.patches.push(Patch::Proposal(ProposalPatch {
                id: id as i64,
                tally: choice.map(|c| (c, weight)),
                updated_height: height,
                ..Default::default()
            }));
        }
        "ProposalTallied" => {
            let id = field_u64(&f, "proposal_id").unwrap_or(0);
            m.touched.proposals.insert(id);
            m.patches.push(Patch::Proposal(ProposalPatch {
                id: id as i64,
                status: field_str(&f, "status").map(|s| s.to_ascii_lowercase()),
                updated_height: height,
                ..Default::default()
            }));
        }
        "ProposalExecuted" => {
            let id = field_u64(&f, "proposal_id").unwrap_or(0);
            let ok = f.get("ok").and_then(Value::as_bool).unwrap_or(true);
            m.touched.proposals.insert(id);
            m.touched.asset_registered = true;
            m.patches.push(Patch::Proposal(ProposalPatch {
                id: id as i64,
                status: Some(if ok { "executed" } else { "failed" }.into()),
                executed_ok: Some(ok),
                updated_height: height,
                tx_id: scope.tx_id.clone(),
                ..Default::default()
            }));
        }
        "ParamChanged" => {
            let key = field_str(&f, "key").unwrap_or_default();
            let to = field_amount(&f, "value").unwrap_or(0);
            let from = ctx.params.insert(key.clone(), to);
            m.patches.push(Patch::Param(ParamRow {
                height,
                tx_index: scope.tx_index,
                event_index,
                timestamp,
                key,
                from,
                to,
                tx_id: scope.tx_id.clone(),
            }));
        }
        "EpochAdvanced" => {
            m.touched.epoch_advanced = true;
            m.patches.push(Patch::Epoch(EpochRow {
                epoch: field_u64(&f, "epoch").unwrap_or(0) as i64,
                start_height: height,
                validator_count: field_u64(&f, "validators").unwrap_or(0) as i32,
            }));
        }
        _ => {}
    }
    let _ = amount_of;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ReceiptRow;

    fn addr(n: u8) -> String {
        format!("{:02x}", n).repeat(32)
    }

    fn receipt(index: u32, signer: &str, events: Vec<Value>) -> ReceiptRow {
        ReceiptRow {
            index,
            tx_id: format!("{index:064x}"),
            signer: signer.into(),
            ok: true,
            error: None,
            events,
            timestamp: Some(61_000),
        }
    }

    fn ctx() -> Ctx {
        let mut c = Ctx::default();
        c.markets
            .insert("BTC-KUSD".into(), ("BTC.BTC".into(), "KUSD".into()));
        c
    }

    #[test]
    fn lightning_events_become_transfers_and_outbound_patches() {
        let mut c = ctx();
        let b = BlockData {
            height: 9,
            timestamp: Some(61_000),
            receipts: vec![
                receipt(
                    0,
                    &addr(7),
                    vec![
                        json!({"LightningDepositCredited": {"owner": addr(2), "observer": addr(7), "amount": "25000", "payment_hash": "cd".repeat(32)}}),
                    ],
                ),
                receipt(
                    1,
                    &addr(2),
                    vec![
                        json!({"WithdrawalQueued": {"outbound_id": 3, "owner": addr(2), "asset": "BTC.BTC", "amount": "12000", "to": "lnbcrt1..."}}),
                        json!({"LightningPayoutAssigned": {"outbound_id": 3, "observer": addr(7)}}),
                    ],
                ),
                receipt(
                    2,
                    &addr(7),
                    vec![
                        json!({"LightningPayoutSettled": {"outbound_id": 3, "observer": addr(7), "fee_paid": "5"}}),
                    ],
                ),
                receipt(
                    3,
                    &addr(7),
                    vec![json!({"LightningPayoutFailed": {"outbound_id": 4, "refunded": "60"}})],
                ),
            ],
            actions: None,
            ..Default::default()
        };
        let m = materialize(&b, &mut c, 0);
        let kinds: Vec<&str> = m.transfers.iter().map(|t| t.kind.as_str()).collect();
        assert_eq!(kinds, vec!["lightning_deposit", "lightning_payout"]);
        assert_eq!(
            (
                m.transfers[0].to.as_deref(),
                m.transfers[0].amount,
                m.transfers[0].asset.as_str()
            ),
            (Some(addr(2).as_str()), 25_000, "BTC.BTC")
        );
        assert_eq!(
            (m.transfers[1].from.as_deref(), m.transfers[1].amount),
            (Some(addr(2).as_str()), 12_000)
        );
        let outbounds: Vec<&OutboundPatch> = m
            .patches
            .iter()
            .filter_map(|p| {
                if let Patch::Outbound(o) = p {
                    Some(o)
                } else {
                    None
                }
            })
            .collect();
        assert!(outbounds.iter().any(|o| o.id == 3
            && o.status.as_deref() == Some("confirmed")
            && o.confirmed_height == Some(9)));
        assert!(outbounds
            .iter()
            .any(|o| o.id == 4 && o.status.as_deref() == Some("failed") && o.refunded == Some(60)));
        assert!(m.touched.outbounds.contains(&3) && m.touched.outbounds.contains(&4));
    }

    #[test]
    fn transfer_and_deposit_events_become_transfers() {
        let b = BlockData {
            height: 5,
            timestamp: Some(61_000),
            receipts: vec![receipt(
                0,
                &addr(1),
                vec![
                    json!({"Transferred": {"from": addr(1), "to": addr(2), "asset": "KEEL", "amount": 7}}),
                    json!({"DepositObserved": {"chain": "BTC", "tx_hash": "ab".repeat(32), "votes": 3}}),
                    json!({"DepositCredited": {"owner": addr(2), "asset": "BTC.BTC", "amount": "50000000"}}),
                ],
            )],
            actions: None,
            ..Default::default()
        };
        let m = materialize(&b, &mut ctx(), 0);
        assert_eq!(m.block.tx_count, 1);
        assert_eq!(m.transfers.len(), 2);
        assert_eq!(
            m.transfers[0],
            TransferRow {
                height: 5,
                tx_index: 0,
                event_index: 0,
                leg: 0,
                tx_id: Some(format!("{:064x}", 0)),
                timestamp: 61_000,
                asset: "KEEL".into(),
                amount: 7,
                from: Some(addr(1)),
                to: Some(addr(2)),
                kind: "transfer".into()
            }
        );
        assert_eq!(m.transfers[1].kind, "deposit");
        assert_eq!(m.transfers[1].amount, 50_000_000);
        assert_eq!(m.transfers[1].from, None);
        // Deposit keyed by the observed tx hash, credited by the same receipt.
        let deposits: Vec<&DepositPatch> = m
            .patches
            .iter()
            .filter_map(|p| {
                if let Patch::Deposit(d) = p {
                    Some(d)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(deposits.len(), 2);
        assert_eq!(deposits[0].key, format!("BTC:{}", "ab".repeat(32)));
        assert_eq!(deposits[1].key, deposits[0].key);
        assert_eq!(deposits[1].status, "credited");
        // Kind inferred from events without an actions route.
        assert_eq!(m.txs[0].kind, "Transfer");
        assert_eq!(m.txs[0].module, "tokens");
        assert_eq!(m.accounts[&addr(1)].tx_count, 1);
        assert_eq!(m.accounts[&addr(2)].tx_count, 0);
        assert!(
            m.events
                .iter()
                .all(|e| e.addresses.iter().all(|a| a != &"ab".repeat(32))),
            "tx hashes are not addresses"
        );
    }

    #[test]
    fn fills_produce_legs_fills_and_candles() {
        let taker = addr(3);
        let maker = addr(4);
        let mut c = ctx();
        c.orders.insert(
            10,
            OrderInfo {
                owner: maker.clone(),
                pair: "BTC-KUSD".into(),
                side: Some("sell".into()),
            },
        );
        let place = json!({"PlaceOrder": {"pair": "BTC-KUSD", "side": "buy", "order_type": "limit", "price": 50_000_000_000u64, "quantity": 200_000u64, "quote_budget": null, "client_id": 9}});
        let b = BlockData {
            height: 8,
            timestamp: Some(125_500),
            receipts: vec![receipt(
                0,
                &taker,
                vec![
                    json!({"OrderAccepted": {"order_id": 11, "owner": taker, "pair": "BTC-KUSD", "resting": 0}}),
                    json!({"OrderFilled": {"order_id": 11, "maker_order_id": 10, "pair": "BTC-KUSD", "price": 50_000_000_000u64, "quantity": 100_000u64, "quote": 50_000_000u64, "fee": 100}}),
                    json!({"OrderFilled": {"order_id": 11, "maker_order_id": null, "pair": "BTC-KUSD", "price": 51_000_000_000u64, "quantity": 100_000u64, "quote": 51_000_000u64, "fee": 100}}),
                ],
            )],
            actions: Some(vec![ActionRow {
                index: 0,
                tx_id: format!("{:064x}", 0),
                signer: taker.clone(),
                nonce: Some(4),
                action: place,
            }]),
            ..Default::default()
        };
        let m = materialize(&b, &mut c, 0);
        assert_eq!(m.txs[0].kind, "PlaceOrder");
        assert_eq!(m.txs[0].module, "markets");
        assert_eq!(m.txs[0].nonce, Some(4));
        assert_eq!(m.fills.len(), 2);
        let f0 = &m.fills[0];
        assert_eq!(f0.taker.as_deref(), Some(taker.as_str()));
        assert_eq!(f0.maker.as_deref(), Some(maker.as_str()));
        assert_eq!(f0.taker_side.as_deref(), Some("buy"));
        assert_eq!(f0.fee_asset.as_deref(), Some("BTC.BTC"));
        assert_eq!(
            m.fills[1].maker.as_deref(),
            Some(SYSTEM_ADDR),
            "synthetic house maker"
        );
        // Legs: base maker→taker, quote taker→maker, fee taker→system; twice.
        assert_eq!(m.transfers.len(), 6);
        assert_eq!(
            (
                m.transfers[0].kind.as_str(),
                m.transfers[0].asset.as_str(),
                m.transfers[0].from.as_deref(),
                m.transfers[0].to.as_deref()
            ),
            (
                "fill_base",
                "BTC.BTC",
                Some(maker.as_str()),
                Some(taker.as_str())
            )
        );
        assert_eq!(
            (
                m.transfers[1].kind.as_str(),
                m.transfers[1].asset.as_str(),
                m.transfers[1].amount
            ),
            ("fill_quote", "KUSD", 50_000_000)
        );
        assert_eq!(
            (m.transfers[2].kind.as_str(), m.transfers[2].amount),
            ("fill_fee", 100)
        );
        // One 1m candle for the minute starting at 120_000.
        assert_eq!(m.candles.len(), 1);
        let c1 = &m.candles[&("BTC-KUSD".to_string(), 120_000)];
        assert_eq!(
            *c1,
            Candle {
                o: 50_000_000_000,
                h: 51_000_000_000,
                l: 50_000_000_000,
                c: 51_000_000_000,
                v: 200_000,
                qv: 101_000_000,
                n: 2
            }
        );
        // Order patches: accept (with side/price from the action) + 3 fill deltas.
        let orders: Vec<&OrderPatch> = m
            .patches
            .iter()
            .filter_map(|p| {
                if let Patch::Order(o) = p {
                    Some(o)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(orders[0].side.as_deref(), Some("buy"));
        assert_eq!(orders[0].price, Some(50_000_000_000));
        assert_eq!(orders[0].status.as_deref(), Some("filled"));
        assert_eq!(
            orders
                .iter()
                .filter(|o| o.id == 10)
                .map(|o| o.filled_delta)
                .sum::<u128>(),
            100_000
        );
        assert_eq!(
            orders
                .iter()
                .filter(|o| o.id == 11)
                .map(|o| o.filled_delta)
                .sum::<u128>(),
            200_000
        );
    }

    #[test]
    fn candle_merge_keeps_open_and_takes_last_close() {
        let mut a = Candle::default();
        a.add(10, 1, 10);
        a.add(12, 1, 12);
        let mut b = Candle::default();
        b.add(9, 2, 18);
        a.merge(&b);
        assert_eq!(
            a,
            Candle {
                o: 10,
                h: 12,
                l: 9,
                c: 9,
                v: 4,
                qv: 40,
                n: 3
            }
        );
        assert_eq!(bucket_1m(125_500), 120_000);
        assert_eq!(bucket_1m(120_000), 120_000);
    }

    #[test]
    fn trade_lifecycle_and_withdrawal() {
        let buyer = addr(5);
        let seller = addr(6);
        let mut c = ctx();
        c.offers.insert(
            1,
            OfferInfo {
                owner: seller.clone(),
                asset: "BTC.BTC".into(),
                fiat_currency: Some("USD".into()),
            },
        );
        c.outbounds.insert(
            0,
            OutboundInfo {
                owner: buyer.clone(),
                asset: "BTC.BTC".into(),
                amount: 20_000_000,
            },
        );
        let b = BlockData {
            height: 20,
            timestamp: Some(200_000),
            receipts: vec![
                receipt(
                    0,
                    &buyer,
                    vec![
                        json!({"TradeStarted": {"trade_id": 7, "offer_id": 1, "buyer": buyer, "seller": seller, "amount": 5_000_000u64}}),
                    ],
                ),
                receipt(
                    1,
                    &seller,
                    vec![
                        json!({"TradeReleased": {"trade_id": 7, "to_buyer": 5_000_000u64, "fee": 50_000u64}}),
                    ],
                ),
                receipt(
                    2,
                    &addr(9),
                    vec![
                        json!({"OutboundConfirmed": {"outbound_id": 0, "tx_hash": "CD".repeat(32)}}),
                    ],
                ),
            ],
            events: vec![json!({"EpochAdvanced": {"epoch": 3, "validators": 4}})],
            actions: None,
            ..Default::default()
        };
        let m = materialize(&b, &mut c, 0);
        let kinds: Vec<&str> = m.transfers.iter().map(|t| t.kind.as_str()).collect();
        assert_eq!(kinds, vec!["trade_release", "trade_fee", "withdrawal"]);
        assert_eq!(m.transfers[0].from.as_deref(), Some(seller.as_str()));
        assert_eq!(m.transfers[0].to.as_deref(), Some(buyer.as_str()));
        assert_eq!(m.transfers[2].amount, 20_000_000);
        let trades: Vec<&TradePatch> = m
            .patches
            .iter()
            .filter_map(|p| {
                if let Patch::Trade(t) = p {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(trades[0].fiat_currency.as_deref(), Some("USD"));
        assert_eq!(trades[0].status.as_deref(), Some("funded"));
        assert_eq!(trades[1].status.as_deref(), Some("released"));
        assert_eq!(trades[1].fee, Some(50_000));
        assert!(m.touched.epoch_advanced);
        assert_eq!(m.events.last().unwrap().tx_index, -1, "block-level event");
        assert_eq!(m.block.event_count, 4);
        assert_eq!(m.txs[1].kind, "ReleaseTrade");
        assert_eq!(m.txs[2].kind, "ObserveOutbound");
    }

    #[test]
    fn empty_block_carries_previous_timestamp() {
        let b = BlockData {
            height: 3,
            ..Default::default()
        };
        let m = materialize(&b, &mut ctx(), 4_242);
        assert_eq!(m.block.timestamp, 4_242);
        assert!(!m.block.timestamp_exact);
    }

    #[test]
    fn action_kinds_map_to_modules() {
        assert_eq!(
            action_kind(&json!("ClaimRewards")),
            ("ClaimRewards".into(), "staking".into())
        );
        assert_eq!(
            action_kind(&json!({"SetParam": {"key": "taker_fee_bps", "value": 5}})),
            ("SetParam".into(), "gov".into())
        );
        assert_eq!(
            action_kind(&json!({"FutureAction": {"x": 1}})),
            ("FutureAction".into(), "unknown".into())
        );
        assert_eq!(action_kind(&json!(null)).0, "unknown");
    }
}
