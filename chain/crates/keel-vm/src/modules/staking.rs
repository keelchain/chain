//! Staking: validator, observer and arbitrator bonds, delegation, unbonding,
//! epochs and reward distribution.
//!
//! Money model: a bond is a transfer from the owner's `deposit` into their
//! restricted `stake_bond` / `observer_bond` / `arbitrator_bond` account.
//! Unbonding keeps the coins in the bond account (they stop counting for
//! power and votes) until `params.unbonding_blocks` later, when `end_block`
//! releases them back to `deposit`. Delegations sit in the DELEGATOR's
//! `stake_bond` account and add power to the validator.
//!
//! Membership vs bonds: the observer and arbitrator SETS are governance
//! (or genesis) decisions; bonding makes an address eligible and gives it
//! something to lose. `is_observer` / `is_arbitrator` answer membership.
//!
//! Rewards: every epoch the system `validator_rewards` balance per asset is
//! paid out directly to `deposit` accounts (observers first, then
//! validators and their delegators pro rata to power). There is nothing to
//! claim, so `ClaimRewards` is refused with an explanatory error.
//!
//! Slashing burns `bps` of the slashed address's OWN bonds (self bond,
//! observer bond, arbitrator bond) and jails it as a validator. Delegations
//! to a slashed validator are not slashed in this version.

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, Bond, Role};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{mul_div_floor, Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::tokens;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Validator {
    pub consensus_key: [u8; 32],
    pub self_bond: Amount,
    pub delegated: Amount,
    pub jailed: bool,
    pub joined_epoch: u64,
}

impl Validator {
    pub fn power(&self) -> Amount {
        if self.jailed {
            0
        } else {
            self.self_bond.saturating_add(self.delegated)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Unbonding {
    pub owner: Address,
    pub role: Role,
    /// For `Role::Validator` entries created by `Undelegate`, the validator
    /// the stake was delegated to (`None` for a validator's own bond).
    pub validator: Option<Address>,
    pub amount: Amount,
    pub release_height: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct StakingState {
    pub validators: BTreeMap<Address, Validator>,
    pub observer_bonds: BTreeMap<Address, Amount>,
    pub arbitrator_bonds: BTreeMap<Address, Amount>,
    /// Governance/genesis-elected observer-signers.
    pub observers: BTreeSet<Address>,
    /// Attestations (count) needed for vault quorum.
    pub observer_threshold: u32,
    /// Governance/genesis-elected arbitrators.
    pub arbitrators: BTreeSet<Address>,
    /// delegator -> validator -> amount (active, not unbonding).
    pub delegations: BTreeMap<Address, BTreeMap<Address, Amount>>,
    /// release height -> entries.
    pub unbonding: BTreeMap<u64, Vec<Unbonding>>,
    pub epoch: u64,
}

pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Validator => "validator",
        Role::Observer => "observer",
        Role::Arbitrator => "arbitrator",
    }
}

fn bond_type(role: Role) -> &'static str {
    match role {
        Role::Validator => "stake_bond",
        Role::Observer => "observer_bond",
        Role::Arbitrator => "arbitrator_bond",
    }
}

pub fn bond_key(owner: Address, native: &Asset, role: Role) -> AccountKey {
    AccountKey::new(owner, native.clone(), bond_type(role)).expect("bond types are in the catalog")
}

fn min_bond(state: &State, role: Role) -> Amount {
    match role {
        Role::Validator => state.params.min_validator_bond,
        Role::Observer => state.params.min_observer_bond,
        Role::Arbitrator => state.params.min_arbitrator_bond,
    }
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::Bond(b) => bond(state, signer, tx_id, b),
        Action::Unbond { role, amount } => unbond(state, ctx, signer, *role, *amount),
        Action::Delegate { validator, amount } => {
            delegate(state, signer, tx_id, *validator, *amount)
        }
        Action::Undelegate { validator, amount } => {
            undelegate(state, ctx, signer, *validator, *amount)
        }
        Action::ClaimRewards => Err(VmError::Invalid(
            "rewards are auto-distributed to deposit accounts at every epoch".into(),
        )),
        _ => Err(VmError::Invalid("not a staking action".into())),
    }
}

fn current_bond(state: &State, who: &Address, role: Role) -> Amount {
    match role {
        Role::Validator => state
            .staking
            .validators
            .get(who)
            .map(|v| v.self_bond)
            .unwrap_or(0),
        Role::Observer => state.staking.observer_bonds.get(who).copied().unwrap_or(0),
        Role::Arbitrator => state
            .staking
            .arbitrator_bonds
            .get(who)
            .copied()
            .unwrap_or(0),
    }
}

fn bond(
    state: &mut State,
    signer: Address,
    tx_id: &[u8; 32],
    b: &Bond,
) -> Result<Vec<Event>, VmError> {
    if b.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let native = state.tokens.native.clone();
    let current = current_bond(state, &signer, b.role);
    let total = current
        .checked_add(b.amount)
        .ok_or_else(|| VmError::Invalid("overflow".into()))?;
    if total < min_bond(state, b.role) {
        return Err(VmError::Invalid(format!(
            "{} bond below minimum",
            role_name(b.role)
        )));
    }
    if b.role == Role::Validator
        && !state.staking.validators.contains_key(&signer)
        && b.consensus_key.is_none()
    {
        return Err(VmError::Invalid(
            "a new validator needs a consensus key".into(),
        ));
    }
    state.ledger.post(
        &format!("bond:{}", tokens::hex(tx_id)),
        TxType::Bond,
        None,
        None,
        vec![
            Record::debit(tokens::deposit_key(signer, &native), b.amount),
            Record::credit(bond_key(signer, &native, b.role), b.amount),
        ],
    )?;
    let epoch = state.staking.epoch;
    match b.role {
        Role::Validator => {
            let v = state
                .staking
                .validators
                .entry(signer)
                .or_insert_with(|| Validator {
                    consensus_key: b.consensus_key.unwrap_or([0u8; 32]),
                    self_bond: 0,
                    delegated: 0,
                    jailed: false,
                    joined_epoch: epoch,
                });
            if let Some(k) = b.consensus_key {
                v.consensus_key = k;
            }
            v.self_bond = total;
        }
        Role::Observer => {
            state.staking.observer_bonds.insert(signer, total);
        }
        Role::Arbitrator => {
            state.staking.arbitrator_bonds.insert(signer, total);
        }
    }
    Ok(vec![Event::Bonded {
        owner: signer,
        role: role_name(b.role).into(),
        amount: b.amount,
    }])
}

fn queue_unbonding(state: &mut State, height: u64, entry: Unbonding) -> u64 {
    let release = height.saturating_add(state.params.unbonding_blocks);
    state
        .staking
        .unbonding
        .entry(release)
        .or_default()
        .push(Unbonding {
            release_height: release,
            ..entry
        });
    release
}

fn unbond(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    role: Role,
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let current = current_bond(state, &signer, role);
    if amount > current {
        return Err(VmError::Invalid("unbond exceeds bonded amount".into()));
    }
    let left = current - amount;
    // Either keep at least the minimum or leave entirely.
    if left > 0 && left < min_bond(state, role) {
        return Err(VmError::Invalid(
            "remaining bond would be below minimum; unbond all".into(),
        ));
    }
    match role {
        Role::Validator => {
            let v = state
                .staking
                .validators
                .get_mut(&signer)
                .ok_or_else(|| VmError::NotFound("validator".into()))?;
            v.self_bond = left;
            if left == 0 && v.delegated == 0 {
                state.staking.validators.remove(&signer);
            }
        }
        Role::Observer => {
            if left == 0 {
                state.staking.observer_bonds.remove(&signer);
            } else {
                state.staking.observer_bonds.insert(signer, left);
            }
        }
        Role::Arbitrator => {
            if left == 0 {
                state.staking.arbitrator_bonds.remove(&signer);
            } else {
                state.staking.arbitrator_bonds.insert(signer, left);
            }
        }
    }
    let release = queue_unbonding(
        state,
        ctx.height,
        Unbonding {
            owner: signer,
            role,
            validator: None,
            amount,
            release_height: 0,
        },
    );
    Ok(vec![Event::Unbonded {
        owner: signer,
        role: role_name(role).into(),
        amount,
        at_height: release,
    }])
}

fn delegate(
    state: &mut State,
    signer: Address,
    tx_id: &[u8; 32],
    validator: Address,
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    if signer == validator {
        return Err(VmError::Invalid(
            "bond as a validator instead of delegating to yourself".into(),
        ));
    }
    let v = state
        .staking
        .validators
        .get(&validator)
        .ok_or_else(|| VmError::NotFound("validator".into()))?;
    if v.jailed {
        return Err(VmError::Invalid("validator is jailed".into()));
    }
    let native = state.tokens.native.clone();
    state.ledger.post(
        &format!("delegate:{}", tokens::hex(tx_id)),
        TxType::Bond,
        None,
        None,
        vec![
            Record::debit(tokens::deposit_key(signer, &native), amount),
            Record::credit(bond_key(signer, &native, Role::Validator), amount),
        ],
    )?;
    let d = state
        .staking
        .delegations
        .entry(signer)
        .or_default()
        .entry(validator)
        .or_insert(0);
    *d = d.saturating_add(amount);
    let v = state
        .staking
        .validators
        .get_mut(&validator)
        .expect("checked");
    v.delegated = v.delegated.saturating_add(amount);
    Ok(vec![Event::Delegated {
        owner: signer,
        validator,
        amount,
    }])
}

fn undelegate(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    validator: Address,
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let current = state
        .staking
        .delegations
        .get(&signer)
        .and_then(|m| m.get(&validator))
        .copied()
        .unwrap_or(0);
    if amount > current {
        return Err(VmError::Invalid("undelegate exceeds delegation".into()));
    }
    let left = current - amount;
    let by = state.staking.delegations.entry(signer).or_default();
    if left == 0 {
        by.remove(&validator);
    } else {
        by.insert(validator, left);
    }
    if by.is_empty() {
        state.staking.delegations.remove(&signer);
    }
    let mut remove_validator = false;
    if let Some(v) = state.staking.validators.get_mut(&validator) {
        v.delegated = v.delegated.saturating_sub(amount);
        remove_validator = v.self_bond == 0 && v.delegated == 0;
    }
    if remove_validator {
        state.staking.validators.remove(&validator);
    }
    let release = queue_unbonding(
        state,
        ctx.height,
        Unbonding {
            owner: signer,
            role: Role::Validator,
            validator: Some(validator),
            amount,
            release_height: 0,
        },
    );
    Ok(vec![Event::Undelegated {
        owner: signer,
        validator,
        amount,
        at_height: release,
    }])
}

pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = release_unbonding(state, ctx);
    let len = state.params.epoch_length_blocks.max(1);
    if ctx.height > 0 && ctx.height.is_multiple_of(len) {
        state.staking.epoch += 1;
        let epoch = state.staking.epoch;
        events.extend(distribute_rewards(state, epoch));
        events.push(Event::EpochAdvanced {
            epoch,
            validators: validator_set(state).len() as u32,
        });
    }
    events
}

fn release_unbonding(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = Vec::new();
    let due: Vec<u64> = state
        .staking
        .unbonding
        .range(..=ctx.height)
        .map(|(h, _)| *h)
        .collect();
    let native = state.tokens.native.clone();
    for h in due {
        let Some(entries) = state.staking.unbonding.remove(&h) else {
            continue;
        };
        for (i, u) in entries.into_iter().enumerate() {
            let posted = state.ledger.post(
                &format!("unbond:{}:{}:{}", h, i, u.owner),
                TxType::Unbond,
                None,
                None,
                vec![
                    Record::debit(bond_key(u.owner, &native, u.role), u.amount),
                    Record::credit(tokens::deposit_key(u.owner, &native), u.amount),
                ],
            );
            if posted.is_ok() {
                events.push(Event::BondReleased {
                    owner: u.owner,
                    role: role_name(u.role).into(),
                    amount: u.amount,
                });
            }
        }
    }
    events
}

/// Pay the system `validator_rewards` balance of every asset out to
/// observers (equal split of `observer_reward_bps`) and validators plus
/// their delegators (pro rata to power). Rounding dust stays in the pool.
fn distribute_rewards(state: &mut State, epoch: u64) -> Vec<Event> {
    let mut events = Vec::new();
    let assets: Vec<Asset> = state.tokens.assets.keys().cloned().collect();
    let observers: Vec<Address> = state.staking.observers.iter().copied().collect();
    let set = validator_set_addresses(state);
    let total_power: Amount = set.iter().map(|(_, p)| *p).fold(0, Amount::saturating_add);
    for asset in assets {
        let pool_key = tokens::system_key(&asset, "validator_rewards");
        let pool = state.ledger.balance(&pool_key).max(0) as Amount;
        if pool == 0 {
            continue;
        }
        let mut credits: BTreeMap<Address, Amount> = BTreeMap::new();
        let observer_share = if observers.is_empty() {
            0
        } else {
            mul_div_floor(pool, state.params.observer_reward_bps as Amount, 10_000).unwrap_or(0)
        };
        if observer_share > 0 {
            let each = observer_share / observers.len() as Amount;
            if each > 0 {
                for o in &observers {
                    *credits.entry(*o).or_insert(0) += each;
                }
            }
        }
        let validator_share = pool.saturating_sub(observer_share);
        if validator_share > 0 && total_power > 0 {
            for (validator, power) in &set {
                let share = mul_div_floor(validator_share, *power, total_power).unwrap_or(0);
                if share == 0 {
                    continue;
                }
                // Delegators take their pro-rata slice; the validator keeps
                // the rest (self-bond share plus rounding dust).
                let mut paid_to_delegators: Amount = 0;
                for (delegator, by) in &state.staking.delegations {
                    if let Some(d) = by.get(validator) {
                        let slice = mul_div_floor(share, *d, *power).unwrap_or(0);
                        if slice > 0 {
                            *credits.entry(*delegator).or_insert(0) += slice;
                            paid_to_delegators += slice;
                        }
                    }
                }
                let own = share.saturating_sub(paid_to_delegators);
                if own > 0 {
                    *credits.entry(*validator).or_insert(0) += own;
                }
            }
        }
        let total: Amount = credits.values().fold(0, |a, b| a.saturating_add(*b));
        if total == 0 {
            continue;
        }
        let mut records = vec![Record::debit(pool_key, total)];
        for (who, amount) in &credits {
            records.push(Record::credit(tokens::deposit_key(*who, &asset), *amount));
        }
        let ext = format!("epoch:{epoch}:rewards:{asset}");
        if state
            .ledger
            .post(
                &ext,
                TxType::RewardDistribute,
                Some(&format!("epoch:{epoch}")),
                None,
                records,
            )
            .is_ok()
        {
            events.push(Event::RewardsDistributed {
                epoch,
                asset: asset.clone(),
                amount: total,
            });
        }
    }
    events
}

/// Genesis hook: seed validators, observers and arbitrators. Bonds are
/// issued straight into the bond accounts.
pub fn genesis(
    state: &mut State,
    validators: &[crate::genesis::GenesisValidator],
    observers: &[Address],
    observer_threshold: u32,
    arbitrators: &[Address],
) {
    let native = state.tokens.native.clone();
    for (i, v) in validators.iter().enumerate() {
        if v.bond > 0 {
            state
                .ledger
                .post(
                    &format!("genesis:validator:{i}"),
                    TxType::Genesis,
                    Some("genesis"),
                    None,
                    vec![
                        Record::debit(tokens::system_key(&native, "issuance"), v.bond),
                        Record::credit(bond_key(v.address, &native, Role::Validator), v.bond),
                    ],
                )
                .expect("genesis bond");
        }
        state.staking.validators.insert(
            v.address,
            Validator {
                consensus_key: v.consensus_key,
                self_bond: v.bond,
                delegated: 0,
                jailed: false,
                joined_epoch: 0,
            },
        );
    }
    state.staking.observers = observers.iter().copied().collect();
    state.staking.observer_threshold = observer_threshold.max(1);
    state.staking.arbitrators = arbitrators.iter().copied().collect();
}

/// Is `who` an elected observer-signer?
pub fn is_observer(state: &State, who: &Address) -> bool {
    state.staking.observers.contains(who)
}

/// Is `who` an elected arbitrator?
pub fn is_arbitrator(state: &State, who: &Address) -> bool {
    state.staking.arbitrators.contains(who)
}

/// Current arbitrator set.
pub fn arbitrators(state: &State) -> Vec<Address> {
    state.staking.arbitrators.iter().copied().collect()
}

/// Governance hooks.
pub fn set_arbitrators(state: &mut State, members: &[Address]) {
    state.staking.arbitrators = members.iter().copied().collect();
}

pub fn set_observers(state: &mut State, members: &[Address], threshold: u32) {
    state.staking.observers = members.iter().copied().collect();
    state.staking.observer_threshold = threshold.max(1).min(members.len().max(1) as u32);
}

/// Observer set and the attestation threshold (count) for vault quorum.
pub fn observers(state: &State) -> (Vec<Address>, u32) {
    (
        state.staking.observers.iter().copied().collect(),
        state.staking.observer_threshold.max(1),
    )
}

/// Bonded KEEL voting weight of `who`: own bonds in every role plus
/// delegations received. Unbonding stake does not count.
pub fn voting_weight(state: &State, who: &Address) -> Amount {
    let s = &state.staking;
    let mut w: Amount = 0;
    if let Some(v) = s.validators.get(who) {
        w = w.saturating_add(v.self_bond).saturating_add(v.delegated);
    }
    w = w.saturating_add(s.observer_bonds.get(who).copied().unwrap_or(0));
    w = w.saturating_add(s.arbitrator_bonds.get(who).copied().unwrap_or(0));
    w
}

/// Total bonded KEEL across all roles and delegations (governance quorum
/// base). Unbonding stake does not count.
pub fn total_bonded(state: &State) -> Amount {
    let s = &state.staking;
    let validators = s
        .validators
        .values()
        .map(|v| v.self_bond.saturating_add(v.delegated))
        .fold(0, Amount::saturating_add);
    let observers = s
        .observer_bonds
        .values()
        .fold(0u128, |a, b| a.saturating_add(*b));
    let arbitrators = s
        .arbitrator_bonds
        .values()
        .fold(0u128, |a, b| a.saturating_add(*b));
    validators
        .saturating_add(observers)
        .saturating_add(arbitrators)
}

/// Slash `bps` of `who`'s own bonds to the system `burn` account and jail
/// the validator. Returns the amount burned.
pub fn slash(state: &mut State, who: &Address, bps: u32, reason: &str) -> Amount {
    let native = state.tokens.native.clone();
    let mut burned: Amount = 0;
    let mut cuts: Vec<(Role, Amount)> = Vec::new();
    if let Some(v) = state.staking.validators.get(who) {
        cuts.push((
            Role::Validator,
            mul_div_floor(v.self_bond, bps as Amount, 10_000).unwrap_or(0),
        ));
    }
    if let Some(b) = state.staking.observer_bonds.get(who) {
        cuts.push((
            Role::Observer,
            mul_div_floor(*b, bps as Amount, 10_000).unwrap_or(0),
        ));
    }
    if let Some(b) = state.staking.arbitrator_bonds.get(who) {
        cuts.push((
            Role::Arbitrator,
            mul_div_floor(*b, bps as Amount, 10_000).unwrap_or(0),
        ));
    }
    let height = state.height;
    for (role, amount) in cuts {
        if amount == 0 {
            continue;
        }
        let ext = format!(
            "slash:{height}:{who}:{}:{}",
            role_name(role),
            state.ledger.next_seq()
        );
        let posted = state.ledger.post(
            &ext,
            TxType::Slash,
            None,
            None,
            vec![
                Record::debit(bond_key(*who, &native, role), amount),
                Record::credit(tokens::system_key(&native, "burn"), amount),
            ],
        );
        if posted.is_err() {
            continue;
        }
        burned = burned.saturating_add(amount);
        match role {
            Role::Validator => {
                if let Some(v) = state.staking.validators.get_mut(who) {
                    v.self_bond = v.self_bond.saturating_sub(amount);
                }
            }
            Role::Observer => {
                if let Some(b) = state.staking.observer_bonds.get_mut(who) {
                    *b = b.saturating_sub(amount);
                }
            }
            Role::Arbitrator => {
                if let Some(b) = state.staking.arbitrator_bonds.get_mut(who) {
                    *b = b.saturating_sub(amount);
                }
            }
        }
    }
    if let Some(v) = state.staking.validators.get_mut(who) {
        v.jailed = true;
    }
    let _ = reason;
    burned
}

/// Active validators as (address, power): the top `max_validators` by
/// power, ties broken by address; jailed and zero-power excluded.
pub fn validator_set_addresses(state: &State) -> Vec<(Address, Amount)> {
    let mut all: Vec<(Address, Amount)> = state
        .staking
        .validators
        .iter()
        .map(|(a, v)| (*a, v.power()))
        .filter(|(_, p)| *p > 0)
        .collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    all.truncate(state.params.max_validators as usize);
    all
}

/// The active validator set as (consensus key, bonded power), ordered by
/// power then address. Consensus reads this at epoch boundaries.
pub fn validator_set(state: &State) -> Vec<([u8; 32], Amount)> {
    validator_set_addresses(state)
        .into_iter()
        .map(|(a, p)| (state.staking.validators[&a].consensus_key, p))
        .collect()
}
