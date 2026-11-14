//! Clients as fee recipients and payers.
//!
//! A client is an attester: the account that vouched for a user's KYC tier
//! (`Attest`). Two things follow from that link, both settled on chain:
//!
//! - **Retail fees.** The client sets its own fee schedule (`SetClientFee`,
//!   basis points on P2P releases, order-book fills and withdrawals, each
//!   capped by governance). When one of its attested accounts pays a
//!   protocol fee, the retail fee is charged to the same payer in the same
//!   block and credited to the client's deposit account. The retail leg is
//!   taken from what the payer's deposit account holds, up to the amount
//!   due, so a release or a fill is never blocked by it.
//! - **Usage.** A client whose account requests a deposit address or queues
//!   a withdrawal pays the governance-set usage price in KEEL, through the
//!   ordinary fee split. A client without KEEL cannot have new addresses or
//!   withdrawals issued to its users; that is its bill.
//!
//! - **Waived protocol fees (0.2.0).** An account attested by a live client
//!   pays no protocol fee on fills, escrow releases, dispute rulings and
//!   the withdrawal flat fee; the client pays a KEEL price per flow
//!   instead (`usage_fill_keel`, `usage_release_keel`, `usage_ruling_keel`,
//!   and the existing per-outbound price). Those charges never block the
//!   user: what the client cannot pay is owed and collected from its next
//!   KEEL, and the rails (addresses, withdrawals) stay closed meanwhile.
//!   With the fee waived the retail caps no longer apply.
//!
//! Governance knobs live here as `clients.*` parameters (`SetParam` and
//! `ParamChange` route them), so `Params` keeps its layout.

use crate::{
    modules::{fees, tokens},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::ClientFee;
use keel_ledger::{Record, TxType};
use keel_types::{mul_div_floor, Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ClientParams {
    /// Caps on a client's retail schedule, in basis points.
    pub fee_cap_p2p_bps: u32,
    pub fee_cap_taker_bps: u32,
    pub fee_cap_withdraw_bps: u32,
    /// KEEL (smallest units) a client pays per deposit address issued to
    /// one of its accounts, and per withdrawal queued. 0 = free.
    pub usage_address_keel: u128,
    pub usage_outbound_keel: u128,
    /// The epoch buyback skips an asset whose best ask is more than this
    /// above the last price.
    pub buyback_max_slippage_bps: u32,
    /// Balances below this USD value are not swept.
    pub buyback_dust_usd_micro: u64,
    /// KEEL per 30-day period for a dedicated node (informational for the
    /// gateway; paid by transfer to the treasury).
    pub service_dedicated_keel_per_period: u128,
    /// 1: accounts attested by a live client pay no protocol fee on fills,
    /// releases, rulings and withdrawals; the client pays the per-flow
    /// usage prices below instead and its retail schedule is uncapped.
    /// 0: the pre-0.2.0 pricing (protocol fee plus capped retail).
    pub protocol_fee_waived: u32,
    /// KEEL (smallest units) a client pays per order that filled, per
    /// escrow release and per dispute ruling of its accounts. 0 = free.
    pub usage_fill_keel: u128,
    pub usage_release_keel: u128,
    pub usage_ruling_keel: u128,
}

impl Default for ClientParams {
    fn default() -> Self {
        Self {
            fee_cap_p2p_bps: 200,
            fee_cap_taker_bps: 50,
            fee_cap_withdraw_bps: 100,
            usage_address_keel: 0,
            usage_outbound_keel: 0,
            buyback_max_slippage_bps: 500,
            buyback_dust_usd_micro: 1_000_000,
            service_dedicated_keel_per_period: 0,
            protocol_fee_waived: 1,
            usage_fill_keel: 0,
            usage_release_keel: 0,
            usage_ruling_keel: 0,
        }
    }
}

/// The `ClientParams` layout of snapshot schemas 2 and 3 (frozen).
#[derive(Clone, Debug, BorshDeserialize)]
pub struct ClientParamsV3 {
    pub fee_cap_p2p_bps: u32,
    pub fee_cap_taker_bps: u32,
    pub fee_cap_withdraw_bps: u32,
    pub usage_address_keel: u128,
    pub usage_outbound_keel: u128,
    pub buyback_max_slippage_bps: u32,
    pub buyback_dust_usd_micro: u64,
    pub service_dedicated_keel_per_period: u128,
}

/// The `ClientsState` layout of snapshot schemas 2 and 3 (frozen).
#[derive(Clone, Debug, BorshDeserialize)]
pub struct ClientsStateV3 {
    pub params: ClientParamsV3,
    pub attested_by: BTreeMap<Address, Address>,
    pub fees: BTreeMap<Address, ClientFee>,
    pub earned: BTreeMap<(Address, Asset), Amount>,
    pub usage_paid: BTreeMap<(Address, Asset), Amount>,
    pub last_buyback: BTreeMap<Asset, (u64, Amount, Amount)>,
}

impl From<ClientsStateV3> for ClientsState {
    fn from(v: ClientsStateV3) -> Self {
        let d = ClientParams::default();
        ClientsState {
            params: ClientParams {
                fee_cap_p2p_bps: v.params.fee_cap_p2p_bps,
                fee_cap_taker_bps: v.params.fee_cap_taker_bps,
                fee_cap_withdraw_bps: v.params.fee_cap_withdraw_bps,
                usage_address_keel: v.params.usage_address_keel,
                usage_outbound_keel: v.params.usage_outbound_keel,
                buyback_max_slippage_bps: v.params.buyback_max_slippage_bps,
                buyback_dust_usd_micro: v.params.buyback_dust_usd_micro,
                service_dedicated_keel_per_period: v.params.service_dedicated_keel_per_period,
                protocol_fee_waived: d.protocol_fee_waived,
                usage_fill_keel: d.usage_fill_keel,
                usage_release_keel: d.usage_release_keel,
                usage_ruling_keel: d.usage_ruling_keel,
            },
            attested_by: v.attested_by,
            fees: v.fees,
            earned: v.earned,
            usage_paid: v.usage_paid,
            last_buyback: v.last_buyback,
            usage_owed: BTreeMap::new(),
            waived_trades: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ClientsState {
    pub params: ClientParams,
    /// Subject -> the attester that last vouched for it.
    pub attested_by: BTreeMap<Address, Address>,
    /// Client -> its retail schedule.
    pub fees: BTreeMap<Address, ClientFee>,
    /// (client, asset) -> retail fees earned so far.
    pub earned: BTreeMap<(Address, Asset), Amount>,
    /// (client, asset) -> usage paid so far.
    pub usage_paid: BTreeMap<(Address, Asset), Amount>,
    /// Asset -> (height, spent, KEEL bought) of the last buyback.
    pub last_buyback: BTreeMap<Asset, (u64, Amount, Amount)>,
    /// Client -> KEEL usage it could not pay when it fell due (schema 6).
    /// Collected from the client's next KEEL; while it is owed, no new
    /// addresses or withdrawals are issued to the client's accounts.
    pub usage_owed: BTreeMap<Address, Amount>,
    /// Trade id -> the client billed at release because the seller's
    /// protocol fee was waived at trade start (schema 6).
    pub waived_trades: BTreeMap<u64, Address>,
}

pub const PARAM_KEYS: &[&str] = &[
    "clients.fee_cap_p2p_bps",
    "clients.fee_cap_taker_bps",
    "clients.fee_cap_withdraw_bps",
    "clients.usage_address_keel",
    "clients.usage_outbound_keel",
    "clients.buyback_max_slippage_bps",
    "clients.buyback_dust_usd_micro",
    "clients.service_dedicated_keel_per_period",
    "clients.protocol_fee_waived",
    "clients.usage_fill_keel",
    "clients.usage_release_keel",
    "clients.usage_ruling_keel",
];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    let p = &state.clients.params;
    Some(match key {
        "clients.fee_cap_p2p_bps" => p.fee_cap_p2p_bps as u128,
        "clients.fee_cap_taker_bps" => p.fee_cap_taker_bps as u128,
        "clients.fee_cap_withdraw_bps" => p.fee_cap_withdraw_bps as u128,
        "clients.usage_address_keel" => p.usage_address_keel,
        "clients.usage_outbound_keel" => p.usage_outbound_keel,
        "clients.buyback_max_slippage_bps" => p.buyback_max_slippage_bps as u128,
        "clients.buyback_dust_usd_micro" => p.buyback_dust_usd_micro as u128,
        "clients.service_dedicated_keel_per_period" => p.service_dedicated_keel_per_period,
        "clients.protocol_fee_waived" => p.protocol_fee_waived as u128,
        "clients.usage_fill_keel" => p.usage_fill_keel,
        "clients.usage_release_keel" => p.usage_release_keel,
        "clients.usage_ruling_keel" => p.usage_ruling_keel,
        _ => return None,
    })
}

/// `SetParam` / `ParamChange` for the `clients.*` keys.
pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    let bps = |v: u128| -> Result<u32, VmError> {
        u32::try_from(v)
            .ok()
            .filter(|b| *b <= 10_000)
            .ok_or_else(|| VmError::Invalid(format!("{key} must be within 0..=10000 bps")))
    };
    let p = &mut state.clients.params;
    match key {
        "clients.fee_cap_p2p_bps" => p.fee_cap_p2p_bps = bps(value)?,
        "clients.fee_cap_taker_bps" => p.fee_cap_taker_bps = bps(value)?,
        "clients.fee_cap_withdraw_bps" => p.fee_cap_withdraw_bps = bps(value)?,
        "clients.usage_address_keel" => p.usage_address_keel = value,
        "clients.usage_outbound_keel" => p.usage_outbound_keel = value,
        "clients.buyback_max_slippage_bps" => p.buyback_max_slippage_bps = bps(value)?,
        "clients.buyback_dust_usd_micro" => {
            p.buyback_dust_usd_micro =
                u64::try_from(value).map_err(|_| VmError::Invalid("too large".into()))?
        }
        "clients.service_dedicated_keel_per_period" => p.service_dedicated_keel_per_period = value,
        "clients.protocol_fee_waived" => {
            if value > 1 {
                return Err(VmError::Invalid(format!("{key} must be 0 or 1")));
            }
            p.protocol_fee_waived = value as u32;
        }
        "clients.usage_fill_keel" => p.usage_fill_keel = value,
        "clients.usage_release_keel" => p.usage_release_keel = value,
        "clients.usage_ruling_keel" => p.usage_ruling_keel = value,
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// `SetClientFee`: an attester's retail schedule. Capped by governance
/// until the client keeps all its fees (`protocol_fee_waived`), after
/// which the schedule is the client's own business.
pub fn apply_set_fee(
    state: &mut State,
    height: u64,
    signer: Address,
    fee: &ClientFee,
) -> Result<Vec<Event>, VmError> {
    if !state.attest.attesters.contains(&signer) {
        return Err(VmError::Unauthorized);
    }
    let p = &state.clients.params;
    let uncapped = active(state, height) && p.protocol_fee_waived == 1;
    if !uncapped
        && (fee.p2p_bps > p.fee_cap_p2p_bps
            || fee.taker_bps > p.fee_cap_taker_bps
            || fee.withdraw_bps > p.fee_cap_withdraw_bps)
    {
        return Err(VmError::Invalid(format!(
            "retail fee above the caps ({} / {} / {} bps)",
            p.fee_cap_p2p_bps, p.fee_cap_taker_bps, p.fee_cap_withdraw_bps
        )));
    }
    state.clients.fees.insert(signer, fee.clone());
    Ok(vec![Event::ClientFeeSet {
        client: signer,
        p2p_bps: fee.p2p_bps,
        taker_bps: fee.taker_bps,
        withdraw_bps: fee.withdraw_bps,
    }])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    P2p,
    Taker,
    Withdraw,
}

impl Flow {
    fn name(self) -> &'static str {
        match self {
            Flow::P2p => "p2p",
            Flow::Taker => "taker",
            Flow::Withdraw => "withdraw",
        }
    }
}

/// The client that collects retail fees from `payer`, if any.
pub fn client_of(state: &State, payer: &Address) -> Option<Address> {
    state.clients.attested_by.get(payer).copied()
}

/// Whether the 0.2.0 pricing (waived protocol fees, per-flow usage) is
/// live at `height`.
pub fn active(state: &State, height: u64) -> bool {
    super::liquidity::active(state, height)
}

/// The client billed for `payer`'s flows: its attester, while the
/// attestation is unexpired. `client_of` never expires, which is right for
/// retail fees the client agreed with its user but not for billing.
pub fn billing_client_of(state: &State, payer: &Address, now_secs: u64) -> Option<Address> {
    let client = client_of(state, payer)?;
    let meta = state.account_ref(payer)?;
    (meta.tier_expires_at > now_secs).then_some(client)
}

/// `Some(client)` when the chain waives its protocol fee for `payer` and
/// bills that client instead: the pricing is active and switched on, the
/// payer has a live client, and the client is not paying for itself.
pub fn waives_fee(state: &State, height: u64, payer: &Address, now_secs: u64) -> Option<Address> {
    if !active(state, height) || state.clients.params.protocol_fee_waived != 1 {
        return None;
    }
    let client = billing_client_of(state, payer, now_secs)?;
    (client != *payer).then_some(client)
}

/// Charge `payer`'s retail fee for `flow` on `base_amount` of `asset` and
/// credit it to its client, up to what the payer's deposit account holds.
/// One balanced posting under `group`; nothing happens when the payer has
/// no client, the client has no schedule, or the fee rounds to zero.
#[allow(clippy::too_many_arguments)]
pub fn retail(
    state: &mut State,
    external_id: &str,
    tx_type: TxType,
    group: Option<&str>,
    payer: Address,
    asset: &Asset,
    base_amount: Amount,
    flow: Flow,
) -> Result<Vec<Event>, VmError> {
    let Some(client) = client_of(state, &payer) else {
        return Ok(Vec::new());
    };
    if client == payer {
        return Ok(Vec::new());
    }
    let Some(schedule) = state.clients.fees.get(&client) else {
        return Ok(Vec::new());
    };
    let bps = match flow {
        Flow::P2p => schedule.p2p_bps,
        Flow::Taker => schedule.taker_bps,
        Flow::Withdraw => schedule.withdraw_bps,
    };
    let due = mul_div_floor(base_amount, bps as Amount, 10_000).unwrap_or(0);
    let available = state
        .ledger
        .balance(&tokens::deposit_key(payer, asset))
        .max(0) as Amount;
    let amount = due.min(available);
    if amount == 0 {
        return Ok(Vec::new());
    }
    state.ledger.post(
        external_id,
        tx_type,
        group,
        None,
        vec![
            Record::debit(tokens::deposit_key(payer, asset), amount),
            Record::credit(tokens::deposit_key(client, asset), amount),
        ],
    )?;
    let e = state
        .clients
        .earned
        .entry((client, asset.clone()))
        .or_insert(0);
    *e = e.saturating_add(amount);
    Ok(vec![Event::ClientFeePaid {
        client,
        payer,
        asset: asset.clone(),
        amount,
        flow: flow.name().to_string(),
    }])
}

/// The usage price a client pays when one of its accounts uses a custody
/// service (`kind`: address | outbound). Paid in KEEL through the fee
/// split; an unfunded client fails the user's action.
pub fn charge_usage(
    state: &mut State,
    user: Address,
    kind: &str,
    tx_id: &[u8; 32],
) -> Result<Vec<Event>, VmError> {
    let Some(client) = client_of(state, &user) else {
        return Ok(Vec::new());
    };
    charge_usage_to(state, client, kind, tx_id)
}

/// Charge the usage price of `kind` to `client` directly (custody
/// services are billed to the custodian whoever signed).
///
/// `address` and `outbound` are the rails: an unfunded client, or one with
/// usage owed, fails the user's action. `fill`, `release` and `ruling`
/// are the flows whose protocol fee the chain waived for the client's
/// user: those never block the user; what the client cannot pay is
/// recorded as owed and collected from its next KEEL.
pub fn charge_usage_to(
    state: &mut State,
    client: Address,
    kind: &str,
    tx_id: &[u8; 32],
) -> Result<Vec<Event>, VmError> {
    let p = &state.clients.params;
    let (price, soft) = match kind {
        "address" => (p.usage_address_keel, false),
        // A float swap is rails usage like a payout, priced the same until
        // it gets its own knob.
        "outbound" | "swap" => (p.usage_outbound_keel, false),
        "rotation" => (state.pools.params.usage_rotation_keel, false),
        "fill" => (p.usage_fill_keel, true),
        "release" => (p.usage_release_keel, true),
        "ruling" => (p.usage_ruling_keel, true),
        _ => (0, true),
    };
    let native = state.tokens.native.clone();
    let mut events = Vec::new();
    // Settle what is owed first; the rails stay closed until it is paid.
    let owed = state.clients.usage_owed.get(&client).copied().unwrap_or(0);
    if owed > 0 {
        let have = state
            .ledger
            .balance(&tokens::deposit_key(client, &native))
            .max(0) as Amount;
        let pay = owed.min(have);
        if pay > 0 {
            fees::collect(
                state,
                &format!("usage:owed:{}", tokens::hex(tx_id)),
                TxType::BudgetPurchase,
                None,
                tokens::deposit_key(client, &native),
                &native,
                pay,
            )?;
            let left = owed - pay;
            if left == 0 {
                state.clients.usage_owed.remove(&client);
            } else {
                state.clients.usage_owed.insert(client, left);
            }
            let u = state
                .clients
                .usage_paid
                .entry((client, native.clone()))
                .or_insert(0);
            *u = u.saturating_add(pay);
            events.push(Event::UsageCharged {
                client,
                asset: native.clone(),
                amount: pay,
                kind: "owed".into(),
            });
        }
        if !soft && state.clients.usage_owed.contains_key(&client) {
            return Err(VmError::Invalid(format!(
                "client {} owes usage; no {kind} until it is paid",
                client.to_hex()
            )));
        }
    }
    if price == 0 {
        return Ok(events);
    }
    let paid = fees::collect(
        state,
        &format!("usage:{kind}:{}", tokens::hex(tx_id)),
        TxType::BudgetPurchase,
        None,
        tokens::deposit_key(client, &native),
        &native,
        price,
    );
    match paid {
        Ok(()) => {
            let u = state
                .clients
                .usage_paid
                .entry((client, native.clone()))
                .or_insert(0);
            *u = u.saturating_add(price);
            events.push(Event::UsageCharged {
                client,
                asset: native,
                amount: price,
                kind: kind.to_string(),
            });
            Ok(events)
        }
        Err(_) if soft => {
            let o = state.clients.usage_owed.entry(client).or_insert(0);
            *o = o.saturating_add(price);
            events.push(Event::UsageOwed {
                client,
                amount: price,
                kind: kind.to_string(),
            });
            Ok(events)
        }
        Err(_) => Err(VmError::Invalid(format!(
            "client {} cannot pay the {kind} usage price of {price} KEEL units",
            client.to_hex()
        ))),
    }
}

/// Epoch boundary: collect usage owed from clients that hold KEEL again.
pub fn end_block(state: &mut State, ctx: &crate::context::BlockContext) -> Vec<Event> {
    if !active(state, ctx.height) {
        return Vec::new();
    }
    let len = state.params.epoch_length_blocks.max(1);
    if ctx.height == 0 || !ctx.height.is_multiple_of(len) {
        return Vec::new();
    }
    let mut events = Vec::new();
    let native = state.tokens.native.clone();
    let owing: Vec<(Address, Amount)> = state
        .clients
        .usage_owed
        .iter()
        .map(|(c, a)| (*c, *a))
        .collect();
    for (client, owed) in owing {
        let have = state
            .ledger
            .balance(&tokens::deposit_key(client, &native))
            .max(0) as Amount;
        let pay = owed.min(have);
        if pay == 0 {
            continue;
        }
        if fees::collect(
            state,
            &format!("usage:owed:{}:{}", ctx.height, client),
            TxType::BudgetPurchase,
            None,
            tokens::deposit_key(client, &native),
            &native,
            pay,
        )
        .is_err()
        {
            continue;
        }
        let left = owed - pay;
        if left == 0 {
            state.clients.usage_owed.remove(&client);
        } else {
            state.clients.usage_owed.insert(client, left);
        }
        let u = state
            .clients
            .usage_paid
            .entry((client, native.clone()))
            .or_insert(0);
        *u = u.saturating_add(pay);
        events.push(Event::UsageCharged {
            client,
            asset: native.clone(),
            amount: pay,
            kind: "owed".into(),
        });
    }
    events
}
