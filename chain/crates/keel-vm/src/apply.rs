//! Admission and dispatch. Per action: signature, chain id, nonce, pause,
//! budget (or observer authorization), then the module. Nonce and budget
//! are consumed even when the module refuses the action, so a failed
//! action can neither be replayed nor spam for free.

use crate::{
    context::BlockContext,
    modules::{
        attest, budgets, clients, custody, disputes, gov, lightning, markets, p2p, sessions,
        stable, staking, tokens, treasury, vaults,
    },
    receipt::{Event, Receipt, VmError},
    state::State,
};
use keel_actions::{budget::BudgetError, Action, SignedAction};
use keel_crypto::sha256;
use rayon::prelude::*;

pub use keel_actions::budget::BudgetError as AdmissionBudgetError;

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
    let p = &state.params.budget;
    let limit = if sa.envelope.action.is_cancel() {
        meta.budget.cancel_limit(p)
    } else {
        meta.budget.limit(p)
    };
    if meta.budget.used >= limit {
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
    end.extend(tokens::end_block(state, ctx));
    end.extend(budgets::end_block(state, ctx));
    end.extend(lightning::end_block(state, ctx));
    end.extend(markets::end_block(state, ctx));
    end.extend(p2p::end_block(state, ctx));
    end.extend(disputes::end_block(state, ctx));
    end.extend(vaults::end_block(state, ctx));
    end.extend(custody::end_block(state, ctx));
    end.extend(stable::end_block(state, ctx));
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
    if state.is_paused(action.module()) {
        return Err(VmError::Paused(action.module().into()));
    }
    if action.is_observer_action() {
        if !staking::is_observer(state, &signer) {
            return Err(VmError::Unauthorized);
        }
    } else {
        let p = state.params.budget.clone();
        let meta = state.account(signer);
        meta.budget
            .spend(&p, ctx.height, ctx.timestamp, action.is_cancel())
            .map_err(|e| match e {
                BudgetError::Exhausted => VmError::BudgetExhausted,
                BudgetError::BlockCap => VmError::BlockCap,
            })?;
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
        Action::MintStable { .. } | Action::BurnStable { .. } => {
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
        Action::SetParam { key, value } => gov::set_param(state, signer, key, *value),
        Action::SetClientFee(fee) => clients::apply_set_fee(state, signer, fee),
        Action::RegisterCustodyVault(_)
        | Action::RequestCustodyAddress { .. }
        | Action::ObserveCustodyDeposit { .. }
        | Action::WithdrawCustody(_) => custody::apply(state, ctx, signer, &tx_id, action),
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
    };
    if result.is_err() {
        state.ledger.rollback(sp);
    }
    result
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
        let liabilities = state
            .ledger
            .user_liabilities(&asset)
            .saturating_sub(custody_liabilities);
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
