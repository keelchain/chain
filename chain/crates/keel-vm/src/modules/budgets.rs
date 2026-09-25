//! Buying extra action budget with KEEL. Revenue goes through the fee split.

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use keel_ledger::TxType;
use keel_types::Address;

use super::{fees, tokens};

pub fn apply_buy(
    state: &mut State,
    _ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    actions: u64,
) -> Result<Vec<Event>, VmError> {
    if actions == 0 {
        return Err(VmError::Invalid("actions must be > 0".into()));
    }
    let price = state.params.budget.price_per_action;
    let cost = price
        .checked_mul(actions as u128)
        .ok_or_else(|| VmError::Invalid("overflow".into()))?;
    let native = state.tokens.native.clone();
    fees::collect(
        state,
        &format!("budget:{}", tokens::hex(tx_id)),
        TxType::BudgetPurchase,
        None,
        tokens::deposit_key(signer, &native),
        &native,
        cost,
    )?;
    state.account(signer).budget.purchase(actions);
    Ok(vec![Event::BudgetPurchased {
        owner: signer,
        actions,
        paid: cost,
    }])
}

// ---------------- locking KEEL for capacity (2026-09-08) ----------------
//
// The Tron energy model: lock KEEL, receive a daily action allowance in
// proportion, unlock with a delay and get every unit back. Nothing is paid.
// The free base budget is untouched, so a retail trader never locks.

use borsh::{BorshDeserialize, BorshSerialize};
use keel_ledger::{AccountKey, Record};
use keel_types::Amount;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct BudgetsState {
    /// Unlock requests by the block time (ms) they mature at: (owner, amount).
    pub unlocking: BTreeMap<u64, Vec<(Address, Amount)>>,
}

fn lock_key(owner: Address, native: &keel_types::Asset) -> AccountKey {
    AccountKey::new(owner, native.clone(), "budget_lock").expect("catalog type")
}

fn unlocking_key(owner: Address, native: &keel_types::Asset) -> AccountKey {
    AccountKey::new(owner, native.clone(), "budget_unlocking").expect("catalog type")
}

pub fn apply_lock(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let native = state.tokens.native.clone();
    state
        .ledger
        .post(
            &format!("budget-lock:{}", tokens::hex(tx_id)),
            TxType::BudgetLock,
            None,
            None,
            vec![
                Record::debit(tokens::deposit_key(signer, &native), amount),
                Record::credit(lock_key(signer, &native), amount),
            ],
        )
        .map_err(VmError::from)?;
    let p = state.params.budget.clone();
    let meta = state.account(signer);
    meta.budget.lock(&p, ctx.timestamp, amount);
    let locked_total = meta.budget.locked;
    Ok(vec![Event::BudgetLocked {
        owner: signer,
        amount,
        locked_total,
    }])
}

pub fn apply_unlock(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let native = state.tokens.native.clone();
    state
        .ledger
        .post(
            &format!("budget-unlock:{}", tokens::hex(tx_id)),
            TxType::BudgetUnlock,
            None,
            None,
            vec![
                Record::debit(lock_key(signer, &native), amount),
                Record::credit(unlocking_key(signer, &native), amount),
            ],
        )
        .map_err(VmError::from)?;
    let p = state.params.budget.clone();
    let ready_at = ctx
        .timestamp
        .saturating_add(p.unlock_delay_secs.saturating_mul(1_000));
    state
        .account(signer)
        .budget
        .unlock(&p, ctx.timestamp, amount);
    state
        .budgets
        .unlocking
        .entry(ready_at)
        .or_default()
        .push((signer, amount));
    Ok(vec![Event::BudgetUnlockQueued {
        owner: signer,
        amount,
        ready_at,
    }])
}

/// Matured unlocks return to the deposit account.
pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let due: Vec<u64> = state
        .budgets
        .unlocking
        .range(..=ctx.timestamp)
        .map(|(t, _)| *t)
        .collect();
    if due.is_empty() {
        return Vec::new();
    }
    let native = state.tokens.native.clone();
    let mut events = Vec::new();
    for t in due {
        let Some(entries) = state.budgets.unlocking.remove(&t) else {
            continue;
        };
        for (i, (owner, amount)) in entries.into_iter().enumerate() {
            let posted = state.ledger.post(
                &format!("budget-unlock-release:{owner}:{t}:{i}"),
                TxType::BudgetUnlockRelease,
                None,
                None,
                vec![
                    Record::debit(unlocking_key(owner, &native), amount),
                    Record::credit(tokens::deposit_key(owner, &native), amount),
                ],
            );
            if posted.is_ok() {
                events.push(Event::BudgetUnlocked { owner, amount });
            }
        }
    }
    events
}

/// Pending unlocks of one address, oldest first: (ready_at ms, amount).
pub fn pending_unlocks(state: &State, owner: Address) -> Vec<(u64, Amount)> {
    state
        .budgets
        .unlocking
        .iter()
        .flat_map(|(t, v)| {
            v.iter()
                .filter(move |(o, _)| *o == owner)
                .map(move |(_, a)| (*t, *a))
        })
        .collect()
}
