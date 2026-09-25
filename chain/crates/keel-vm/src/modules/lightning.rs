//! Lightning (2026-09-10): fast, cheap BTC deposits and withdrawals
//! through per-observer hot pools next to the cold threshold vault.
//!
//! A Lightning node must keep its channel keys online, so pool funds cannot
//! sit under the vault's threshold key. Instead each observer runs a node,
//! the chain tracks how much BTC that observer holds in it (`balance`),
//! caps it (`lightning_pool_cap_sats`), and the observer's bond backs it.
//! Ledger-wise the pooled BTC is the system `lightning_pool` asset account,
//! counted in reserves like `vault_asset`, so the reserves ≥ liabilities
//! invariant keeps holding across both.
//!
//! - Deposit: the observer issues a BOLT11 invoice whose description binds
//!   the owner (`keel:<address>`); when it settles it reports the preimage.
//!   The VM re-parses the invoice: signed by the observer's registered
//!   node, hash matches the preimage, owner and amount as claimed.
//! - Withdrawal: `Withdraw` with a BOLT11 destination; the chain locks the
//!   amount plus a routing-fee allowance, assigns the payout to the
//!   observer with the most free pool balance, and that observer reports
//!   settlement (preimage) or failure; a missed deadline refunds.
//! - Funding / sweeping: an observer moves vault BTC into its pool with a
//!   quorum-confirmed outbound (`FundLightningPool`) and returns BTC by an
//!   on-chain send to the vault's index-0 address (`AnnounceLightningSweep`
//!   first, then the usual deposit observation).
//!
//! Every limit is a chain parameter, editable from the backoffice.

use crate::{
    context::BlockContext,
    modules::{tokens, vaults},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Chain, LightningDepositObservation, OutboundId, Withdraw};
use keel_crypto::Hash32;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct LightningPool {
    /// Compressed secp256k1 node id (33 bytes).
    pub node_id: Vec<u8>,
    /// Sats this observer holds in Lightning on the chain's books.
    pub balance: Amount,
    /// Sats reserved by assigned, unsettled payouts (amount + fee allowance).
    pub pending_out: Amount,
    /// (day number, sats credited that day) for the daily cap.
    pub credited_today: (u64, Amount),
    pub registered_height: u64,
}

impl LightningPool {
    pub fn available(&self) -> Amount {
        self.balance.saturating_sub(self.pending_out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Assignment {
    pub observer: Address,
    pub deadline_height: u64,
    pub fee_allowance: Amount,
    pub payment_hash: Hash32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct LightningState {
    pub pools: BTreeMap<Address, LightningPool>,
    /// Payment hashes already credited.
    pub credited: BTreeSet<Hash32>,
    /// Outbound id → assigned observer (Lightning payouts).
    pub assignments: BTreeMap<OutboundId, Assignment>,
    /// Outbound id → observer (pool fundings from the vault).
    pub fundings: BTreeMap<OutboundId, Address>,
    /// Announced sweep tx hashes → (observer, amount) awaiting the on-chain deposit.
    pub sweeps: BTreeMap<Hash32, (Address, Amount)>,
}

fn btc() -> Asset {
    vaults::native_asset(Chain::Bitcoin)
}

fn pool_key() -> AccountKey {
    tokens::system_key(&btc(), "lightning_pool")
}

fn escrow_key(owner: Address) -> AccountKey {
    AccountKey::new(owner, btc(), "sendout_escrow").expect("catalog type")
}

fn require_enabled(state: &State) -> Result<(), VmError> {
    if state.params.lightning_enabled == 0 {
        return Err(VmError::Paused("lightning".into()));
    }
    Ok(())
}

fn require_pool<'a>(
    state: &'a mut State,
    observer: &Address,
) -> Result<&'a mut LightningPool, VmError> {
    state
        .lightning
        .pools
        .get_mut(observer)
        .ok_or_else(|| VmError::Invalid("observer has no Lightning node registered".into()))
}

/// Routing-fee allowance locked with a payout: max(bps of amount, floor).
pub fn fee_allowance(state: &State, amount: Amount) -> Amount {
    let bps = amount.saturating_mul(state.params.lightning_max_fee_bps as Amount) / 10_000;
    bps.max(state.params.lightning_min_fee_sats as Amount)
}

pub fn register_node(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    node_id: &[u8],
) -> Result<Vec<Event>, VmError> {
    if node_id.len() != 33 || !(node_id[0] == 0x02 || node_id[0] == 0x03) {
        return Err(VmError::Invalid(
            "node_id must be a 33-byte compressed secp256k1 public key".into(),
        ));
    }
    let entry = state
        .lightning
        .pools
        .entry(signer)
        .or_insert_with(|| LightningPool {
            node_id: Vec::new(),
            balance: 0,
            pending_out: 0,
            credited_today: (0, 0),
            registered_height: ctx.height,
        });
    entry.node_id = node_id.to_vec();
    Ok(vec![Event::LightningNodeRegistered {
        observer: signer,
        node_id: tokens::hex(node_id),
    }])
}

pub fn observe_deposit(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    o: &LightningDepositObservation,
) -> Result<Vec<Event>, VmError> {
    require_enabled(state)?;
    let inv = keel_ln::parse(&o.invoice).map_err(|e| VmError::Invalid(e.to_string()))?;
    let pool = state
        .lightning
        .pools
        .get(&signer)
        .ok_or_else(|| VmError::Invalid("observer has no Lightning node registered".into()))?;
    if inv.payee.as_slice() != pool.node_id.as_slice() {
        return Err(VmError::Unauthorized);
    }
    if !keel_ln::preimage_matches(&o.preimage, &inv.payment_hash) {
        return Err(VmError::Invalid(
            "preimage does not match the invoice".into(),
        ));
    }
    let owner_hex = keel_ln::owner_of_description(&inv.description)
        .ok_or_else(|| VmError::Invalid("invoice is not bound to an KEEL account".into()))?;
    let owner = Address::from_hex(owner_hex)
        .ok_or_else(|| VmError::Invalid("bad owner address in invoice".into()))?;
    if owner.is_system() {
        return Err(VmError::Invalid("cannot credit the system address".into()));
    }
    if o.amount_msat < inv.amount_msat {
        return Err(VmError::Invalid(
            "received less than the invoice amount".into(),
        ));
    }
    let sats = keel_ln::msat_to_sat(o.amount_msat) as Amount;
    if sats == 0 {
        return Err(VmError::Invalid("amount must be at least 1 sat".into()));
    }
    if sats > state.params.lightning_max_deposit_sats as Amount {
        return Err(VmError::Invalid("above lightning_max_deposit_sats".into()));
    }
    if state.lightning.credited.contains(&inv.payment_hash) {
        return Err(VmError::Invalid("invoice already credited".into()));
    }
    let cap = state.params.lightning_pool_cap_sats as Amount;
    let daily_cap = state.params.lightning_daily_cap_sats as Amount;
    let day = ctx.seconds() / 86_400;
    let pool = require_pool(state, &signer)?;
    if pool.balance.saturating_add(sats) > cap {
        return Err(VmError::Invalid(
            "observer's Lightning pool would exceed lightning_pool_cap_sats".into(),
        ));
    }
    if pool.credited_today.0 != day {
        pool.credited_today = (day, 0);
    }
    if daily_cap > 0 && pool.credited_today.1.saturating_add(sats) > daily_cap {
        return Err(VmError::Invalid(
            "above lightning_daily_cap_sats for this observer".into(),
        ));
    }
    let hash_hex = tokens::hex(&inv.payment_hash);
    state.ledger.post(
        &format!("ln:deposit:{hash_hex}"),
        TxType::LightningDeposit,
        None,
        None,
        vec![
            Record::debit(pool_key(), sats),
            Record::credit(tokens::deposit_key(owner, &btc()), sats),
        ],
    )?;
    let pool = require_pool(state, &signer)?;
    pool.balance = pool.balance.saturating_add(sats);
    pool.credited_today.1 = pool.credited_today.1.saturating_add(sats);
    state.lightning.credited.insert(inv.payment_hash);
    Ok(vec![Event::LightningDepositCredited {
        owner,
        observer: signer,
        amount: sats,
        payment_hash: hash_hex,
    }])
}

/// `Withdraw` with a BOLT11 destination (called from `vaults::withdraw`).
pub fn queue_payout(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    w: &Withdraw,
) -> Result<Vec<Event>, VmError> {
    require_enabled(state)?;
    if w.asset != btc() {
        return Err(VmError::Invalid(
            "Lightning withdrawals are BTC only".into(),
        ));
    }
    if state.vaults.halted.contains(&w.asset) {
        return Err(VmError::Paused("outbounds for BTC.BTC".into()));
    }
    let inv = keel_ln::parse(&w.to).map_err(|e| VmError::Invalid(e.to_string()))?;
    let invoice_sats = keel_ln::msat_to_sat(inv.amount_msat) as Amount;
    if invoice_sats == 0 || w.amount != invoice_sats {
        return Err(VmError::Invalid(
            "amount must equal the invoice amount".into(),
        ));
    }
    if w.amount > state.params.lightning_max_withdraw_sats as Amount {
        return Err(VmError::Invalid("above lightning_max_withdraw_sats".into()));
    }
    if inv.expires_at <= ctx.seconds().saturating_add(120) {
        return Err(VmError::Invalid("invoice expires too soon".into()));
    }
    let fee = fee_allowance(state, w.amount);
    let need = w.amount.saturating_add(fee);
    // The observer with the most free pool balance takes it (deterministic).
    let observer = state
        .lightning
        .pools
        .iter()
        .filter(|(_, p)| !p.node_id.is_empty() && p.available() >= need)
        .max_by_key(|(a, p)| (p.available(), std::cmp::Reverse(**a)))
        .map(|(a, _)| *a)
        .ok_or_else(|| {
            VmError::Invalid(
                "no Lightning capacity right now; withdraw to an on-chain address instead".into(),
            )
        })?;
    let id = tokens::hex(tx_id);
    let outbound_id = state.vaults.next_outbound_id;
    let group = format!("outbound:{outbound_id}");
    state.ledger.post(
        &format!("{group}:prepare:{id}"),
        TxType::SendoutPrepare,
        Some(&group),
        None,
        vec![
            Record::debit(tokens::deposit_key(signer, &w.asset), need),
            Record::credit(escrow_key(signer), need),
        ],
    )?;
    state.vaults.next_outbound_id += 1;
    state.vaults.outbounds.insert(
        outbound_id,
        vaults::Outbound {
            id: outbound_id,
            owner: signer,
            asset: w.asset.clone(),
            chain: Chain::Bitcoin,
            to: w.to.clone(),
            amount: w.amount,
            fee_asset: w.asset.clone(),
            fee_estimate: fee,
            status: vaults::OutboundStatus::Batched,
            batch_id: None,
            created_height: ctx.height,
            quorum: Default::default(),
            tx_hash: Some(inv.payment_hash),
            votes: Vec::new(),
        },
    );
    let deadline_height = ctx
        .height
        .saturating_add(state.params.lightning_payout_timeout_blocks.max(1));
    state.lightning.assignments.insert(
        outbound_id,
        Assignment {
            observer,
            deadline_height,
            fee_allowance: fee,
            payment_hash: inv.payment_hash,
        },
    );
    let pool = require_pool(state, &observer)?;
    pool.pending_out = pool.pending_out.saturating_add(need);
    Ok(vec![
        Event::WithdrawalQueued {
            outbound_id,
            owner: signer,
            asset: w.asset.clone(),
            amount: w.amount,
            to: w.to.clone(),
        },
        Event::LightningPayoutAssigned {
            outbound_id,
            observer,
        },
    ])
}

pub fn observe_payout(
    state: &mut State,
    signer: Address,
    outbound_id: OutboundId,
    preimage: Option<Hash32>,
    fee_paid_msat: u64,
    success: bool,
) -> Result<Vec<Event>, VmError> {
    let a = state
        .lightning
        .assignments
        .get(&outbound_id)
        .cloned()
        .ok_or_else(|| VmError::NotFound(format!("lightning payout {outbound_id}")))?;
    if a.observer != signer {
        return Err(VmError::Unauthorized);
    }
    if success {
        let p = preimage
            .ok_or_else(|| VmError::Invalid("a successful payout needs the preimage".into()))?;
        if !keel_ln::preimage_matches(&p, &a.payment_hash) {
            return Err(VmError::Invalid(
                "preimage does not match the invoice".into(),
            ));
        }
        let fee_paid = (keel_ln::msat_to_sat(fee_paid_msat) as Amount).min(a.fee_allowance);
        settle(state, outbound_id, Some(fee_paid))
    } else {
        settle(state, outbound_id, None)
    }
}

/// Settles an assigned payout: `Some(fee)` paid, `None` failed/refunded.
fn settle(
    state: &mut State,
    outbound_id: OutboundId,
    paid: Option<Amount>,
) -> Result<Vec<Event>, VmError> {
    let a = state
        .lightning
        .assignments
        .remove(&outbound_id)
        .ok_or_else(|| VmError::NotFound(format!("lightning payout {outbound_id}")))?;
    let ob = state
        .vaults
        .outbounds
        .get(&outbound_id)
        .cloned()
        .ok_or_else(|| VmError::NotFound(format!("outbound {outbound_id}")))?;
    let need = ob.amount.saturating_add(a.fee_allowance);
    let group = format!("outbound:{outbound_id}");
    let event = match paid {
        Some(fee_paid) => {
            let spent = ob.amount.saturating_add(fee_paid);
            let refund = a.fee_allowance.saturating_sub(fee_paid);
            let mut records = vec![
                Record::debit(escrow_key(ob.owner), spent),
                Record::credit(pool_key(), spent),
            ];
            if refund > 0 {
                records.push(Record::debit(escrow_key(ob.owner), refund));
                records.push(Record::credit(
                    tokens::deposit_key(ob.owner, &btc()),
                    refund,
                ));
            }
            state.ledger.post(
                &format!("{group}:complete"),
                TxType::LightningPayout,
                Some(&group),
                None,
                records,
            )?;
            let pool = require_pool(state, &a.observer)?;
            pool.balance = pool.balance.saturating_sub(spent);
            pool.pending_out = pool.pending_out.saturating_sub(need);
            let out = state
                .vaults
                .outbounds
                .get_mut(&outbound_id)
                .expect("exists");
            out.status = vaults::OutboundStatus::Confirmed;
            Event::LightningPayoutSettled {
                outbound_id,
                observer: a.observer,
                fee_paid,
            }
        }
        None => {
            state.ledger.post(
                &format!("{group}:failed"),
                TxType::SendoutFailed,
                Some(&group),
                None,
                vec![
                    Record::debit(escrow_key(ob.owner), need),
                    Record::credit(tokens::deposit_key(ob.owner, &btc()), need),
                ],
            )?;
            if let Some(pool) = state.lightning.pools.get_mut(&a.observer) {
                pool.pending_out = pool.pending_out.saturating_sub(need);
            }
            let out = state
                .vaults
                .outbounds
                .get_mut(&outbound_id)
                .expect("exists");
            out.status = vaults::OutboundStatus::Failed;
            Event::LightningPayoutFailed {
                outbound_id,
                refunded: need,
            }
        }
    };
    Ok(vec![event])
}

/// Vault BTC → the observer's on-chain Lightning wallet, as a quorum-signed outbound.
pub fn fund_pool(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    amount: Amount,
    to: &str,
) -> Result<Vec<Event>, VmError> {
    require_enabled(state)?;
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    if !vaults::valid_destination(Chain::Bitcoin, to) {
        return Err(VmError::Invalid("bad destination address".into()));
    }
    if state.vaults.active_vault(Chain::Bitcoin).is_none() {
        return Err(VmError::Invalid("no Bitcoin vault registered".into()));
    }
    if state.vaults.halted.contains(&btc()) {
        return Err(VmError::Paused("outbounds for BTC.BTC".into()));
    }
    let cap = state.params.lightning_pool_cap_sats as Amount;
    let in_flight: Amount = state
        .lightning
        .fundings
        .iter()
        .filter(|(_, o)| **o == signer)
        .filter_map(|(id, _)| state.vaults.outbounds.get(id))
        .map(|o| o.amount)
        .fold(0, Amount::saturating_add);
    let pool = require_pool(state, &signer)?;
    if pool
        .balance
        .saturating_add(in_flight)
        .saturating_add(amount)
        > cap
    {
        return Err(VmError::Invalid(
            "pool would exceed lightning_pool_cap_sats".into(),
        ));
    }
    let rate = state.vaults.fee_rate(Chain::Bitcoin);
    let fee_estimate =
        (rate as Amount).saturating_mul(vaults::fee_size(Chain::Bitcoin, true) as Amount);
    let outbound_id = state.vaults.next_outbound_id;
    state.vaults.next_outbound_id += 1;
    state.vaults.outbounds.insert(
        outbound_id,
        vaults::Outbound {
            id: outbound_id,
            owner: Address::SYSTEM,
            asset: btc(),
            chain: Chain::Bitcoin,
            to: to.to_string(),
            amount,
            fee_asset: btc(),
            fee_estimate,
            status: vaults::OutboundStatus::Queued,
            batch_id: None,
            created_height: ctx.height,
            quorum: Default::default(),
            tx_hash: None,
            votes: Vec::new(),
        },
    );
    state.lightning.fundings.insert(outbound_id, signer);
    Ok(vec![Event::LightningPoolFunded {
        observer: signer,
        amount,
        outbound_id,
    }])
}

/// Called by `vaults::observe_outbound` once a funding outbound settles.
pub fn on_funding_settled(
    state: &mut State,
    outbound_id: OutboundId,
    success: bool,
    fee_paid: Amount,
) -> Result<(), VmError> {
    let Some(observer) = state.lightning.fundings.remove(&outbound_id) else {
        return Ok(());
    };
    if !success {
        return Ok(());
    }
    let ob = state
        .vaults
        .outbounds
        .get(&outbound_id)
        .cloned()
        .ok_or_else(|| VmError::NotFound(format!("outbound {outbound_id}")))?;
    let group = format!("outbound:{outbound_id}");
    let mut records = vec![
        Record::credit(tokens::system_key(&btc(), "vault_asset"), ob.amount),
        Record::debit(pool_key(), ob.amount),
    ];
    if fee_paid > 0 {
        // The mining fee leaves the reserves for good.
        records.push(Record::credit(
            tokens::system_key(&btc(), "vault_asset"),
            fee_paid,
        ));
        records.push(Record::debit(
            tokens::system_key(&btc(), "sweep_gas"),
            fee_paid,
        ));
    }
    state.ledger.post(
        &format!("{group}:fund"),
        TxType::LightningPoolFund,
        Some(&group),
        None,
        records,
    )?;
    if let Some(pool) = state.lightning.pools.get_mut(&observer) {
        pool.balance = pool.balance.saturating_add(ob.amount);
    }
    Ok(())
}

pub fn announce_sweep(
    state: &mut State,
    signer: Address,
    tx_hash: Hash32,
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let pool = require_pool(state, &signer)?;
    if pool.available() < amount {
        return Err(VmError::Invalid(
            "sweep exceeds the pool's free balance".into(),
        ));
    }
    if state.lightning.sweeps.contains_key(&tx_hash) {
        return Err(VmError::Invalid("sweep already announced".into()));
    }
    state.lightning.sweeps.insert(tx_hash, (signer, amount));
    Ok(vec![Event::LightningSweepAnnounced {
        observer: signer,
        tx_hash: tokens::hex(&tx_hash),
        amount,
    }])
}

/// Called by `vaults::observe_deposit` for a quorum-confirmed deposit to
/// the vault's index-0 address: the announced sweep returns to the vault.
pub fn on_sweep_deposit(
    state: &mut State,
    tx_hash: &Hash32,
    amount: Amount,
    external_id: &str,
) -> Result<Vec<Event>, VmError> {
    let (observer, announced) = state.lightning.sweeps.remove(tx_hash).ok_or_else(|| {
        VmError::Invalid(
            "deposit to the vault's own address was not announced as a Lightning sweep".into(),
        )
    })?;
    let credited = amount.min(announced);
    state.ledger.post(
        external_id,
        TxType::LightningPoolSweep,
        Some(external_id),
        None,
        vec![
            Record::debit(tokens::system_key(&btc(), "vault_asset"), credited),
            Record::credit(pool_key(), credited),
        ],
    )?;
    if let Some(pool) = state.lightning.pools.get_mut(&observer) {
        pool.balance = pool.balance.saturating_sub(credited);
    }
    Ok(vec![Event::LightningPoolSwept {
        observer,
        amount: credited,
    }])
}

/// Missed payout deadlines refund the owner.
pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let due: Vec<OutboundId> = state
        .lightning
        .assignments
        .iter()
        .filter(|(_, a)| a.deadline_height <= ctx.height)
        .map(|(id, _)| *id)
        .collect();
    let mut events = Vec::new();
    for id in due {
        if let Ok(ev) = settle(state, id, None) {
            events.extend(ev);
        }
    }
    events
}

/// Pools as the RPC shows them.
pub fn pools_view(state: &State) -> Vec<(Address, LightningPool)> {
    state
        .lightning
        .pools
        .iter()
        .map(|(a, p)| (*a, p.clone()))
        .collect()
}
