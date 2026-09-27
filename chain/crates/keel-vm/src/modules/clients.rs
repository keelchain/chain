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
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// `SetClientFee`: an attester's retail schedule, within the caps.
pub fn apply_set_fee(
    state: &mut State,
    signer: Address,
    fee: &ClientFee,
) -> Result<Vec<Event>, VmError> {
    if !state.attest.attesters.contains(&signer) {
        return Err(VmError::Unauthorized);
    }
    let p = &state.clients.params;
    if fee.p2p_bps > p.fee_cap_p2p_bps
        || fee.taker_bps > p.fee_cap_taker_bps
        || fee.withdraw_bps > p.fee_cap_withdraw_bps
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
pub fn charge_usage_to(
    state: &mut State,
    client: Address,
    kind: &str,
    tx_id: &[u8; 32],
) -> Result<Vec<Event>, VmError> {
    let price = match kind {
        "address" => state.clients.params.usage_address_keel,
        "outbound" => state.clients.params.usage_outbound_keel,
        _ => 0,
    };
    if price == 0 {
        return Ok(Vec::new());
    }
    let native = state.tokens.native.clone();
    fees::collect(
        state,
        &format!("usage:{kind}:{}", tokens::hex(tx_id)),
        TxType::BudgetPurchase,
        None,
        tokens::deposit_key(client, &native),
        &native,
        price,
    )
    .map_err(|_| {
        VmError::Invalid(format!(
            "client {} cannot pay the {kind} usage price of {price} KEEL units",
            client.to_hex()
        ))
    })?;
    let u = state
        .clients
        .usage_paid
        .entry((client, native.clone()))
        .or_insert(0);
    *u = u.saturating_add(price);
    Ok(vec![Event::UsageCharged {
        client,
        asset: native,
        amount: price,
        kind: kind.to_string(),
    }])
}
