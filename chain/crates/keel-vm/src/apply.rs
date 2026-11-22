//! Admission and dispatch. Per action: signature, chain id, nonce, pause,
//! activation (the signer holds `budget.activation_keel`), budget (or
//! observer authorization), then the module. Nonce and budget are consumed
//! even when the module refuses the action, so a failed action can neither
//! be replayed nor spam for free.
//!
//! There is no free budget on a network (`budget.base` is 0): an address
//! acts on the capacity its locked KEEL grants, or on what it earned by
//! filling. The way in is the lock itself: a `LockBudget` of at least
//! `budget.free_lock_min_keel` that the signer can pay in full spends no
//! budget (`budgets::is_free_lock`). It cannot be repeated for nothing,
//! because every such lock ties its KEEL up for the unlock delay.
//!
//! An account attested by a client with a running plan has a second source:
//! once its own capacity is used up, its actions come out of the plan's
//! daily pool for the client's users (`clients::sponsor_of`), up to a
//! per-user share. Such an account needs no KEEL at all, so the activation
//! floor does not apply to it while the plan can pay.
//!
//! Fee mode: with `budget.base` at 0 and `budget.action_fee_keel` above 0,
//! an address that has no earned or locked capacity left pays that fee per
//! action instead, through the fee split. The fee is taken before the
//! module runs and stays taken if it refuses. At the default fee of 0 such
//! an address is refused (`BUDGET_EXHAUSTED`).

use crate::{
    context::BlockContext,
    modules::{
        arbitration, attest, budgets, clients, custody, disputes, fees, gov, lightning, limits,
        liquidity, markets, p2p, pools, recovery, rewards, sessions, stable, staking, tokens,
        treasury, vaults,
    },
    receipt::{Event, Receipt, VmError},
    state::State,
};
use keel_actions::{budget::BudgetError, Action, SignedAction};
use keel_crypto::sha256;
use rayon::prelude::*;

pub use keel_actions::budget::BudgetError as AdmissionBudgetError;

/// Actions that only unwind state need no activation KEEL: cancels, and a
/// session key revoking itself (its principal's KEEL is what paid for it;
/// the key itself may hold none).
fn exempt_from_activation(action: &Action, signer: keel_types::Address) -> bool {
    action.is_cancel() || matches!(action, Action::RevokeSessionKey { key } if *key == signer)
}

/// Cheap checks a mempool runs before gossiping an action. Does not mutate.
pub fn check_admission(state: &State, sa: &SignedAction) -> Result<(), VmError> {
    if sa.envelope.chain_id != state.chain_id {
        return Err(VmError::WrongChain);
    }
    if !sa.verify() {
        return Err(VmError::BadSignature);
    }
    let meta = state
        .account_ref(&sa.envelope.signer)
        .cloned()
        .unwrap_or_default();
    if sa.envelope.nonce != meta.nonce {
        return Err(VmError::BadNonce {
            expected: meta.nonce,
            got: sa.envelope.nonce,
        });
    }
    if state.is_paused(sa.envelope.action.module()) {
        return Err(VmError::Paused(sa.envelope.action.module().into()));
    }
    if sa.envelope.action.is_observer_action() {
        if !staking::is_observer(state, &sa.envelope.signer) {
            return Err(VmError::Unauthorized);
        }
        return Ok(());
    }
    if !exempt_from_activation(&sa.envelope.action, sa.envelope.signer) {
        // A session key acts for its principal: the principal's KEEL counts.
        let principal = sessions::principal_of(
            state,
            sa.envelope.signer,
            &sa.envelope.action,
            state.timestamp / 1000,
        )
        .unwrap_or(None)
        .unwrap_or(sa.envelope.signer);
        if clients::sponsor_of(state, &principal, state.timestamp / 1000).is_none() {
            budgets::require_activated(state, principal)?;
        }
    }
    if budgets::is_free_lock(state, sa.envelope.signer, &sa.envelope.action) {
        return Ok(());
    }
    let p = &state.params.budget;
    let limit = if sa.envelope.action.is_cancel() {
        meta.budget.cancel_limit(p)
    } else {
        meta.budget.limit(p)
    };
    if meta.budget.used >= limit && meta.budget.lock_pool == 0 {
        // The signer's client pays from its plan.
        if clients::sponsor_of(state, &sa.envelope.signer, state.timestamp / 1000).is_some() {
            return Ok(());
        }
        // Fee mode: the action is admitted if the signer can pay for it.
        let fee = state.limits.action_fee_keel;
        if limits::fee_mode(state)
            && fee > 0
            && tokens::balance(state, sa.envelope.signer, &state.tokens.native) >= fee
        {
            return Ok(());
        }
        return Err(VmError::BudgetExhausted);
    }
    Ok(())
}

/// Apply a whole block. Returns one receipt per action plus end-of-block
/// events, and updates `state.last_hash`.
pub fn apply_block(
    state: &mut State,
    ctx: &BlockContext,
    actions: &[SignedAction],
) -> (Vec<Receipt>, Vec<Event>) {
    state.height = ctx.height;
    state.timestamp = ctx.timestamp;
    // Signature checks touch no state, so they run in parallel up front;
    // the verdicts are consumed in order, which keeps application
    // deterministic.
    let verified: Vec<bool> = actions.par_iter().map(SignedAction::verify).collect();
    let mut receipts = Vec::with_capacity(actions.len());
    for (i, sa) in actions.iter().enumerate() {
        let (ok, error, events) = match apply_one(state, ctx, sa, verified[i]) {
            Ok(events) => (true, None, events),
            Err(e) => (false, Some(e), Vec::new()),
        };
        receipts.push(Receipt {
            index: i as u32,
            height: ctx.height,
            timestamp: ctx.timestamp,
            tx_id: sa.id(),
            signer: sa.signer(),
            ok,
            error,
            events,
        });
    }
    let mut end = Vec::new();
    end.extend(rewards::end_block(state, ctx));
    end.extend(tokens::end_block(state, ctx));
    end.extend(budgets::end_block(state, ctx));
    end.extend(lightning::end_block(state, ctx));
    end.extend(markets::end_block(state, ctx));
    end.extend(p2p::end_block(state, ctx));
    end.extend(disputes::end_block(state, ctx));
    end.extend(pools::end_block(state, ctx));
    end.extend(vaults::end_block(state, ctx));
    end.extend(custody::end_block(state, ctx));
    end.extend(stable::end_block(state, ctx));
    // The pool's epoch fee and clients' owed usage land in the fee split
    // before the buyback sweeps it into KEEL.
    end.extend(liquidity::end_block(state, ctx));
    end.extend(recovery::end_block(state, ctx));
    end.extend(clients::end_block(state, ctx));
    end.extend(treasury::end_block(state, ctx));
    end.extend(staking::end_block(state, ctx));
    end.extend(gov::end_block(state, ctx));
    end.extend(invariants(state));
    // Incremental commitment: the previous hash chained with everything
    // this block decided (each action id with its verdict, then the
    // end-of-block events). Two nodes that apply the same blocks to the
    // same genesis reach the same chain of hashes; a full-state hash
    // (`State::compute_hash`) remains available for snapshots and audits.
    let mut parts: Vec<Vec<u8>> = Vec::with_capacity(receipts.len() + 3);
    parts.push(state.last_hash.to_vec());
    parts.push(ctx.height.to_be_bytes().to_vec());
    for r in &receipts {
        let mut p = r.tx_id.to_vec();
        p.push(u8::from(r.ok));
        if let Some(e) = &r.error {
            p.extend_from_slice(e.code().as_bytes());
        }
        parts.push(p);
    }
    parts.push(borsh::to_vec(&end).unwrap_or_default());
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    state.last_hash = sha256(&refs);
    (receipts, end)
}

fn apply_one(
    state: &mut State,
    ctx: &BlockContext,
    sa: &SignedAction,
    signature_ok: bool,
) -> Result<Vec<Event>, VmError> {
    if sa.envelope.chain_id != state.chain_id {
        return Err(VmError::WrongChain);
    }
    if !signature_ok {
        return Err(VmError::BadSignature);
    }
    let key = sa.envelope.signer;
    if key.is_system() {
        return Err(VmError::Unauthorized);
    }
    let action = &sa.envelope.action;
    {
        // The nonce belongs to the key that signed (a session key has its own).
        let meta = state.account(key);
        if sa.envelope.nonce != meta.nonce {
            return Err(VmError::BadNonce {
                expected: meta.nonce,
                got: sa.envelope.nonce,
            });
        }
        meta.nonce += 1;
    }
    // A live session key acts as its principal for in-scope actions
    // (2026-09-09); everything below sees the principal.
    let signer = sessions::principal_of(state, key, action, ctx.seconds())?.unwrap_or(key);
    let mut fee_paid: Option<Event> = None;
    if state.is_paused(action.module()) {
        return Err(VmError::Paused(action.module().into()));
    }
    if action.is_observer_action() {
        if !staking::is_observer(state, &signer) {
            return Err(VmError::Unauthorized);
        }
    } else {
        // An account whose client's plan can pay for the action needs no
        // KEEL of its own: the client vouched for it and the plan's pool
        // bounds what all its users can send.
        let sponsor = clients::sponsor_of(state, &signer, ctx.seconds());
        if !exempt_from_activation(action, signer) && sponsor.is_none() {
            budgets::require_activated(state, signer)?;
        }
        let p = state.params.budget.clone();
        let map = |e| match e {
            BudgetError::Exhausted => VmError::BudgetExhausted,
            BudgetError::BlockCap => VmError::BlockCap,
        };
        let spent = if budgets::is_free_lock(state, signer, action) {
            // The first capacity an address gets: only the per-block cap
            // counts against it.
            state.account(signer).budget.take_block_slot(&p, ctx.height)
        } else {
            state
                .account(signer)
                .budget
                .spend(&p, ctx.height, ctx.timestamp, action.is_cancel())
        };
        match (spent, sponsor) {
            (Ok(()), _) => {}
            (Err(BudgetError::Exhausted), Some(client)) => {
                // The signer's own capacity goes first; after it, the
                // client's plan pays.
                state
                    .account(signer)
                    .budget
                    .take_block_slot(&p, ctx.height)
                    .map_err(map)?;
                clients::take_user_action(state, client, signer, ctx.seconds());
            }
            (Err(BudgetError::Exhausted), None) if limits::fee_mode(state) => {
                // Fee mode: pay for the action instead. Taken before the
                // savepoint below, so a refused action still paid.
                let fee = state.limits.action_fee_keel;
                if fee == 0 {
                    return Err(VmError::BudgetExhausted);
                }
                state
                    .account(signer)
                    .budget
                    .take_block_slot(&p, ctx.height)
                    .map_err(map)?;
                let native = state.tokens.native.clone();
                fees::collect(
                    state,
                    &format!("action-fee:{}", tokens::hex(&sa.id())),
                    keel_ledger::TxType::ActionFee,
                    None,
                    tokens::deposit_key(signer, &native),
                    &native,
                    fee,
                )?;
                fee_paid = Some(Event::ActionFeePaid {
                    owner: signer,
                    amount: fee,
                });
            }
            (Err(e), _) => return Err(map(e)),
        }
    }

    let tx_id = sa.id();
    let sp = state.ledger.savepoint();
    let result = match action {
        Action::Transfer(t) => tokens::apply_transfer(state, ctx, signer, &tx_id, t),
        Action::PlaceOrder(_) | Action::CancelOrder { .. } | Action::HouseQuote(_) => {
            markets::apply(state, ctx, signer, &tx_id, action)
        }
        Action::BuyBudget { actions } => budgets::apply_buy(state, ctx, signer, &tx_id, *actions),
        Action::LockBudget { amount } => budgets::apply_lock(state, ctx, signer, &tx_id, *amount),
        Action::UnlockBudget { amount } => {
            budgets::apply_unlock(state, ctx, signer, &tx_id, *amount)
        }
        Action::CreateOffer(_)
        | Action::UpdateOffer { .. }
        | Action::PauseOffer { .. }
        | Action::CloseOffer { .. }
        | Action::StartTrade(_)
        | Action::MarkPaid { .. }
        | Action::ReleaseTrade { .. }
        | Action::CancelTrade { .. } => p2p::apply(state, ctx, signer, &tx_id, action),
        Action::OpenDispute { .. } | Action::SubmitEvidence { .. } | Action::RuleDispute { .. } => {
            disputes::apply(state, ctx, signer, &tx_id, action)
        }
        Action::RequestDepositAddress { .. } | Action::Withdraw(_) => {
            // The user's client pays the usage price for the service.
            vaults::apply(state, ctx, signer, &tx_id, action).and_then(|mut events| {
                let kind = if matches!(action, Action::Withdraw(_)) {
                    "outbound"
                } else {
                    "address"
                };
                events.extend(clients::charge_usage(state, signer, kind, &tx_id)?);
                Ok(events)
            })
        }
        Action::ObserveDeposit(_)
        | Action::ObserveOutbound(_)
        | Action::ReportNetworkFee { .. }
        | Action::RegisterVault(_) => vaults::apply(state, ctx, signer, &tx_id, action),
        Action::MintStable { .. } | Action::BurnStable { .. } | Action::WithdrawStable(_) => {
            stable::apply(state, ctx, signer, &tx_id, action)
        }
        Action::Bond(_)
        | Action::Unbond { .. }
        | Action::Delegate { .. }
        | Action::Undelegate { .. }
        | Action::ClaimRewards => staking::apply(state, ctx, signer, &tx_id, action),
        Action::Propose(_) | Action::Vote { .. } | Action::ExecuteProposal { .. } => {
            gov::apply(state, ctx, signer, &tx_id, action)
        }
        Action::Attest {
            subject,
            tier,
            expires_at,
        } => attest::apply_attest(state, ctx, signer, *subject, *tier, *expires_at),
        Action::SetClientFee(fee) => clients::apply_set_fee(state, ctx.height, signer, fee),
        Action::BuyPlan { tier } => clients::apply_buy_plan(state, ctx, signer, &tx_id, *tier),
        Action::SetPlanRenewal { tier } => clients::apply_set_renewal(state, signer, *tier),
        Action::RegisterCustodyVault(_)
        | Action::RequestCustodyAddress { .. }
        | Action::ObserveCustodyDeposit { .. }
        | Action::WithdrawCustody(_)
        | Action::SwapCustody(_)
        | Action::TransferCustody(_)
        | Action::WithdrawCustodyStable(_)
        | Action::WithdrawCustodyFor { .. }
        | Action::SetCustodyOptions { .. }
        | Action::ReleaseCustodyHold { .. } => custody::apply(state, ctx, signer, &tx_id, action),
        Action::SetOfferArbitrators { .. } | Action::ChooseArbitrator { .. } => {
            arbitration::apply(state, signer, action)
        }
        Action::RotateCustodyVault(_)
        | Action::CancelCustodyRotation { .. }
        | Action::RetireCustodyGeneration { .. } => {
            pools::apply(state, ctx, signer, &tx_id, action)
        }
        Action::AuthorizeSessionKey {
            key: k,
            scope,
            expires_at,
        } => {
            if signer != key {
                // A session key cannot mint session keys.
                Err(VmError::Unauthorized)
            } else {
                sessions::authorize(state, ctx, signer, *k, *scope, *expires_at)
            }
        }
        Action::RevokeSessionKey { key: k } => sessions::revoke(state, key, *k),
        Action::RegisterLightningNode { node_id } => {
            lightning::register_node(state, ctx, signer, node_id)
        }
        Action::ObserveLightningDeposit(o) => lightning::observe_deposit(state, ctx, signer, o),
        Action::ObserveLightningPayout {
            outbound_id,
            preimage,
            fee_paid_msat,
            success,
        } => lightning::observe_payout(
            state,
            signer,
            *outbound_id,
            *preimage,
            *fee_paid_msat,
            *success,
        ),
        Action::FundLightningPool { amount, to } => {
            lightning::fund_pool(state, ctx, signer, *amount, to)
        }
        Action::AnnounceLightningSweep { tx_hash, amount } => {
            lightning::announce_sweep(state, signer, *tx_hash, *amount)
        }
        Action::DepositLiquidity { .. } | Action::WithdrawLiquidity { .. } => {
            liquidity::apply(state, ctx, signer, &tx_id, action)
        }
        Action::RequestRecovery(_) | Action::ObserveRecovery(_) => {
            recovery::apply(state, ctx, signer, &tx_id, action)
        }
    };
    if result.is_err() {
        state.ledger.rollback(sp);
    }
    match (result, fee_paid) {
        (Ok(mut events), Some(fee)) => {
            events.insert(0, fee);
            Ok(events)
        }
        (r, _) => r,
    }
}

/// Block-boundary invariants (docs/plan.md §4): per vault asset,
/// reserves must cover user liabilities. A breach emits an event; the
/// vaults module halts outbounds for that asset when it sees one.
fn invariants(state: &mut State) -> Vec<Event> {
    let mut events = Vec::new();
    let assets: Vec<_> = state
        .tokens
        .assets
        .iter()
        .filter(|(_, info)| matches!(info.kind, tokens::AssetKind::Vault { .. }))
        .map(|(a, _)| a.clone())
        .collect();
    for asset in assets {
        // Client vaults back their own custody balances and are checked
        // on their own (custody::end_block); leave both sides out here.
        let (custody_reserves, custody_liabilities) = custody::totals(state, &asset);
        let reserves = state
            .ledger
            .system_reserves(&asset)
            .saturating_sub(custody_reserves)
            .max(0) as u128;
        // Depositors' share of the liquidity pool's vault assets sits in
        // a system account `user_liabilities` ignores; count it here.
        let liabilities = state
            .ledger
            .user_liabilities(&asset)
            .saturating_sub(custody_liabilities)
            .saturating_add(liquidity::user_pool_liability(state, state.height, &asset));
        if reserves < liabilities {
            events.push(Event::InvariantBreached {
                asset: asset.clone(),
                reserves,
                liabilities,
            });
            vaults::halt_outbounds(state, &asset);
        }
    }
    events
}
