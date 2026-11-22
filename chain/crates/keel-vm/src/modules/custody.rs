//! Client-owned custody vaults (docs/models.md, Model A). A client that
//! keeps its own keys registers a vault per chain: the observers watch its
//! addresses and attest deposits exactly as for the network vault, but the
//! coins are the client's, so they are recorded as *custody* balances backed
//! by the client's own reserve, never by the network vault, and withdrawals
//! are signed by the client's own signer (`signer_url`). Keel builds,
//! batches and broadcasts; it never holds a share of the key.
//!
//! Money legs (all in one asset):
//!   deposit     custodian vault_asset -> owner custody
//!   withdraw    owner custody         -> owner custody_escrow
//!   confirmed   owner custody_escrow  -> custodian vault_asset
//!               custodian custody (native) -> custodian vault_asset  (network fee)
//!   failed      owner custody_escrow  -> owner custody
//!   swap        owner custody <-> custodian custody, and
//!               custodian deposit <-> owner deposit   (no coins move)
//!
//! Every custody vault is checked at each block against its own reserve:
//! reserve(custodian, asset) >= custody balances it backs. A breach halts
//! that vault's outbounds for the asset until the reserve covers them
//! again (the client tops up), and the explorer shows it.

use crate::{
    context::BlockContext,
    modules::{
        clients, staking, tokens,
        vaults::{self, Outbound, OutboundStatus, PendingDeposit},
    },
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{
    Action, Chain, CustodySwap, CustodyVaultRegistration, DepositObservation, OutboundId, SwapSide,
    Withdraw,
};
use keel_attest::{Attestation, Quorum};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct CustodyVault {
    pub custodian: Address,
    pub chain: Chain,
    pub epoch: u64,
    pub public_key: Vec<u8>,
    pub chain_code: Option<[u8; 32]>,
    pub signer_url: String,
    pub registered_height: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct CustodyState {
    /// (chain, custodian) -> the active vault (re-registration replaces it
    /// with a higher epoch).
    pub vaults: BTreeMap<(Chain, Address), CustodyVault>,
    pub next_deposit_index: BTreeMap<(Chain, Address), u64>,
    /// (chain, custodian, index) -> account the address belongs to.
    pub deposit_owner: BTreeMap<(Chain, Address, u64), Address>,
    /// (account, chain) -> its index in its custodian's vault.
    pub deposit_index_of: BTreeMap<(Address, Chain), u64>,
    /// An account's custody balances are backed by exactly one client.
    pub custodian_of: BTreeMap<Address, Address>,
    pub pending: BTreeMap<[u8; 32], PendingDeposit>,
    pub credited: BTreeSet<(Chain, [u8; 32], u32)>,
    /// Outbounds (in `vaults.outbounds`) drawn from a custody vault.
    pub outbound_vault: BTreeMap<OutboundId, Address>,
    /// Batches (in `vaults.batches`) of a custody vault.
    pub batch_vault: BTreeMap<u64, Address>,
    /// (custodian, asset) -> custody balances it backs (incl. escrow).
    pub liabilities: BTreeMap<(Address, Asset), Amount>,
    pub halted: BTreeSet<(Address, Asset)>,
    /// (custodian, reserve asset) -> that asset held in the custodian's
    /// vault as backing of the unified USDT its accounts hold, in the
    /// reserve asset's units (see "One USDT in custody" below).
    pub stable_backing: BTreeMap<(Address, Asset), Amount>,
    /// Outbounds that pay out redeemed USDT: a failed one is converted
    /// back, so the account is not left holding a per-network asset.
    pub stable_outbounds: BTreeSet<OutboundId>,
    /// Custodians whose users' deposits are credited on hold
    /// (`SetCustodyOptions`), to be released after screening.
    pub hold_deposits: BTreeSet<Address>,
}

impl CustodyState {
    pub fn vault(&self, chain: Chain, custodian: Address) -> Option<&CustodyVault> {
        self.vaults.get(&(chain, custodian))
    }

    pub fn reserve(&self, state: &State, custodian: Address, asset: &Asset) -> i128 {
        state.ledger.balance(&reserve_key(custodian, asset))
    }
}

pub fn reserve_key(custodian: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(custodian, asset.clone(), "vault_asset").expect("catalog type")
}

pub fn custody_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "custody").expect("catalog type")
}

fn escrow_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "custody_escrow").expect("catalog type")
}

/// Spendable custody balance of `owner` in `asset`.
pub fn hold_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "custody_hold").expect("catalog type")
}

fn stable_reserve_key(custodian: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(custodian, asset.clone(), "custody_stable_reserve").expect("catalog type")
}

fn issuance_key(custodian: Address, stable: &Asset) -> AccountKey {
    AccountKey::new(custodian, stable.clone(), "custody_issuance").expect("catalog type")
}

pub fn balance(state: &State, owner: Address, asset: &Asset) -> Amount {
    state.ledger.balance(&custody_key(owner, asset)).max(0) as Amount
}

fn add_liability(state: &mut State, custodian: Address, asset: &Asset, amount: Amount) {
    let l = state
        .custody
        .liabilities
        .entry((custodian, asset.clone()))
        .or_insert(0);
    *l = l.saturating_add(amount);
}

fn sub_liability(state: &mut State, custodian: Address, asset: &Asset, amount: Amount) {
    if let Some(l) = state
        .custody
        .liabilities
        .get_mut(&(custodian, asset.clone()))
    {
        *l = l.saturating_sub(amount);
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
        Action::RegisterCustodyVault(r) => register(state, ctx, signer, r),
        Action::RequestCustodyAddress {
            chain,
            custodian,
            owner,
        } => {
            // The custodian may ask for one of its accounts; nobody else
            // asks for an account that is not their own.
            let account = match owner {
                Some(o) if *o != signer => {
                    if signer != *custodian {
                        return Err(VmError::Unauthorized);
                    }
                    *o
                }
                _ => signer,
            };
            let mut events = request_address(state, account, *chain, *custodian)?;
            events.extend(clients::charge_usage_to(
                state, *custodian, "address", tx_id,
            )?);
            Ok(events)
        }
        Action::ObserveCustodyDeposit {
            custodian,
            observation,
        } => observe_deposit(state, ctx, signer, *custodian, observation),
        Action::WithdrawCustody(w) => {
            let (custodian, mut events) = withdraw(state, ctx, signer, tx_id, w)?;
            events.extend(clients::charge_usage_to(
                state, custodian, "outbound", tx_id,
            )?);
            Ok(events)
        }
        Action::SwapCustody(s) => {
            let (custodian, mut events) = swap(state, signer, tx_id, s)?;
            events.extend(clients::charge_usage_to(state, custodian, "swap", tx_id)?);
            Ok(events)
        }
        Action::TransferCustody(t) => transfer(state, signer, tx_id, t),
        Action::WithdrawCustodyFor {
            owner,
            asset,
            via,
            to,
            amount,
            fee,
        } => {
            // Only the custodian of the account pays out of it.
            let theirs = state.custody.custodian_of.get(owner).copied().or_else(|| {
                state
                    .custody
                    .vaults
                    .keys()
                    .any(|(_, c)| c == owner)
                    .then_some(*owner)
            });
            match theirs {
                Some(c) if c == signer => {}
                Some(_) => return Err(VmError::Unauthorized),
                None => return Err(VmError::NotEnoughFunds),
            }
            // The client's own fee first, as a ledger posting only: if
            // the payout below is refused, it is rolled back with it.
            let mut fee_events = Vec::new();
            if let Some(f) = fee {
                fee_events = transfer(
                    state,
                    signer,
                    tx_id,
                    &keel_actions::CustodyTransfer {
                        legs: vec![keel_actions::CustodyLeg {
                            from: None,
                            to: f.to,
                            asset: asset.clone(),
                            amount: f.amount,
                        }],
                        on_behalf_of: Some(*owner),
                        memo: Some("withdrawal fee".into()),
                    },
                )?;
            }
            let (custodian, mut events) = if *asset == state.tokens.stable {
                let via = via.clone().ok_or_else(|| {
                    VmError::Invalid("USDT needs `via`: the network to pay it on".into())
                })?;
                withdraw_stable(
                    state,
                    ctx,
                    *owner,
                    tx_id,
                    &keel_actions::WithdrawStable {
                        via,
                        to: to.clone(),
                        amount: *amount,
                    },
                )?
            } else {
                withdraw(
                    state,
                    ctx,
                    *owner,
                    tx_id,
                    &Withdraw {
                        asset: asset.clone(),
                        to: to.clone(),
                        amount: *amount,
                    },
                )?
            };
            events.extend(clients::charge_usage_to(
                state, custodian, "outbound", tx_id,
            )?);
            fee_events.append(&mut events);
            Ok(fee_events)
        }
        Action::SetCustodyOptions { hold_deposits } => {
            if !state.custody.vaults.keys().any(|(_, c)| *c == signer) {
                return Err(VmError::Invalid(
                    "only a client with a custody vault sets custody options".into(),
                ));
            }
            if *hold_deposits {
                state.custody.hold_deposits.insert(signer);
            } else {
                state.custody.hold_deposits.remove(&signer);
            }
            Ok(vec![Event::CustodyOptionsSet {
                custodian: signer,
                hold_deposits: *hold_deposits,
            }])
        }
        Action::ReleaseCustodyHold {
            owner,
            asset,
            amount,
            to,
        } => release_hold(state, signer, tx_id, *owner, asset, *amount, *to),
        Action::WithdrawCustodyStable(w) => {
            let (custodian, mut events) = withdraw_stable(state, ctx, signer, tx_id, w)?;
            events.extend(clients::charge_usage_to(
                state, custodian, "outbound", tx_id,
            )?);
            Ok(events)
        }
        _ => Err(VmError::Invalid("not a custody action".into())),
    }
}

fn register(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    r: &CustodyVaultRegistration,
) -> Result<Vec<Event>, VmError> {
    if !state.attest.attesters.contains(&signer) {
        return Err(VmError::Unauthorized);
    }
    if r.public_key.len() != r.chain.key_length() {
        return Err(VmError::Invalid("bad vault public key length".into()));
    }
    if r.chain_code.is_none() {
        return Err(VmError::Invalid(
            "a custody vault needs a chain code".into(),
        ));
    }
    if !valid_signer_url(&r.signer_url) {
        return Err(VmError::Invalid("signer_url must be an http(s) URL".into()));
    }
    if state.custody.vault(r.chain, signer).is_some() {
        // Once registered, a vault changes key only through a rotation,
        // which keeps the old generation watched (`pools`).
        return Err(VmError::Invalid(
            "vault already registered; rotate to a fresh key with RotateCustodyVault".into(),
        ));
    }
    state.custody.vaults.insert(
        (r.chain, signer),
        CustodyVault {
            custodian: signer,
            chain: r.chain,
            epoch: r.epoch,
            public_key: r.public_key.clone(),
            chain_code: r.chain_code,
            signer_url: r.signer_url.clone(),
            registered_height: ctx.height,
        },
    );
    Ok(vec![Event::CustodyVaultRegistered {
        custodian: signer,
        chain: r.chain.as_str().into(),
        epoch: r.epoch,
    }])
}

pub(crate) fn valid_signer_url(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://"))
        && url.len() <= 256
        && url.chars().all(|c| c.is_ascii_graphic())
}

/// A network fee the custodian's vault paid (a payout): from its
/// own native custody balance (its gas tank) or, failing that, booked as
/// its expense until its next top-up settles it.
pub(crate) fn charge_network_fee(
    state: &mut State,
    custodian: Address,
    native: &Asset,
    fee_paid: Amount,
    group: &str,
) -> Result<(), VmError> {
    let from_custody = balance(state, custodian, native) >= fee_paid;
    let debit = if from_custody {
        custody_key(custodian, native)
    } else {
        // Tracked from now on so the reserve check sees the hole.
        add_liability(state, custodian, native, 0);
        expense_key(custodian, native)
    };
    state.ledger.post(
        &format!("{group}:fee"),
        TxType::SendoutComplete,
        Some(group),
        None,
        vec![
            Record::debit(debit, fee_paid),
            Record::credit(reserve_key(custodian, native), fee_paid),
        ],
    )?;
    if from_custody {
        sub_liability(state, custodian, native, fee_paid);
    }
    Ok(())
}

fn require_member(state: &State, signer: Address, custodian: Address) -> Result<(), VmError> {
    if signer == custodian || clients::client_of(state, &signer) == Some(custodian) {
        Ok(())
    } else {
        Err(VmError::Unauthorized)
    }
}

fn request_address(
    state: &mut State,
    signer: Address,
    chain: Chain,
    custodian: Address,
) -> Result<Vec<Event>, VmError> {
    require_member(state, signer, custodian)?;
    if state.custody.vault(chain, custodian).is_none() {
        return Err(VmError::NotFound(format!(
            "custody vault of {} on {}",
            custodian.to_hex(),
            chain.as_str()
        )));
    }
    match state.custody.custodian_of.get(&signer) {
        Some(c) if *c != custodian => {
            return Err(VmError::Invalid(
                "account is already in another client's custody".into(),
            ))
        }
        Some(_) => {}
        None => {
            state.custody.custodian_of.insert(signer, custodian);
        }
    }
    let index = match state.custody.deposit_index_of.get(&(signer, chain)) {
        Some(i) => *i,
        None => {
            let next = state
                .custody
                .next_deposit_index
                .entry((chain, custodian))
                .or_insert(1);
            let index = *next;
            *next += 1;
            state
                .custody
                .deposit_owner
                .insert((chain, custodian, index), signer);
            state
                .custody
                .deposit_index_of
                .insert((signer, chain), index);
            index
        }
    };
    Ok(vec![Event::CustodyAddressAssigned {
        custodian,
        owner: signer,
        chain: chain.as_str().into(),
        index,
    }])
}

fn observe_deposit(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    custodian: Address,
    o: &DepositObservation,
) -> Result<Vec<Event>, VmError> {
    let (observers, threshold) = vaults::require_observer(state, &signer)?;
    if vaults::chain_of(state, &o.asset)? != o.chain {
        return Err(VmError::Invalid(
            "asset is not custodied on that chain".into(),
        ));
    }
    if state.custody.vault(o.chain, custodian).is_none() {
        return Err(VmError::NotFound("custody vault".into()));
    }
    // Index 0 is the vault's own address: the client's own top-ups (its
    // gas and float) land there and belong to the client.
    let owner = if o.deposit_index == 0 {
        custodian
    } else {
        *state
            .custody
            .deposit_owner
            .get(&(o.chain, custodian, o.deposit_index))
            .ok_or_else(|| {
                VmError::NotFound(format!(
                    "custody deposit index {} on {}",
                    o.deposit_index,
                    o.chain.as_str()
                ))
            })?
    };
    if o.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let key = (o.chain, o.tx_hash, o.index);
    if state.custody.credited.contains(&key) {
        return Err(VmError::Invalid("deposit already credited".into()));
    }
    let depth = o.tip_height.saturating_sub(o.external_height) as u32;
    let required = vaults::confirmations(state, o.chain);
    if depth < required {
        return Err(VmError::Invalid(format!(
            "need {required} confirmations, have {depth}"
        )));
    }
    vaults::verify_proof(state, o)?;

    let attestation = Attestation {
        chain: o.chain,
        tx_hash: o.tx_hash,
        index: o.index,
        deposit_index: o.deposit_index,
        amount: o.amount,
        asset: o.asset.clone(),
        external_height: o.external_height,
        external_tx: o.external_tx.clone(),
    };
    let digest = attestation.digest();
    let entry = state
        .custody
        .pending
        .entry(digest)
        .or_insert_with(|| PendingDeposit {
            attestation,
            quorum: Quorum::default(),
            first_height: ctx.height,
            last_height: ctx.height,
            status: vaults::DepositStatus::Pending,
        });
    if entry.status != vaults::DepositStatus::Pending {
        return Err(VmError::Invalid("deposit already resolved".into()));
    }
    if !entry.quorum.vote(signer) {
        return Err(VmError::Invalid("observer already voted".into()));
    }
    entry.last_height = ctx.height;
    let votes = entry.quorum.count_within(&observers);
    let mut events = vec![Event::DepositObserved {
        chain: o.chain.as_str().into(),
        tx_hash: tokens::hex(&o.tx_hash),
        votes,
        external_tx: o.external_tx.clone(),
    }];
    if !entry.quorum.reached(&observers, threshold) {
        return Ok(events);
    }
    entry.status = vaults::DepositStatus::Credited;
    let external_id = format!(
        "custody:{}:{}:{}",
        o.chain.as_str(),
        tokens::hex(&o.tx_hash),
        o.index
    );
    // A client that screens deposits has its users' deposits credited on
    // hold: in the reserve and owed, but not spendable until it releases
    // them. Its own top-ups are never held.
    let held = owner != custodian && state.custody.hold_deposits.contains(&custodian);
    let credit = if held {
        hold_key(owner, &o.asset)
    } else {
        custody_key(owner, &o.asset)
    };
    state.ledger.post(
        &external_id,
        TxType::VaultDeposit,
        None,
        None,
        vec![
            Record::debit(reserve_key(custodian, &o.asset), o.amount),
            Record::credit(credit, o.amount),
        ],
    )?;
    state.custody.credited.insert(key);
    add_liability(state, custodian, &o.asset, o.amount);
    if held {
        events.push(Event::CustodyDepositHeld {
            custodian,
            owner,
            asset: o.asset.clone(),
            amount: o.amount,
            tx_hash: tokens::hex(&o.tx_hash),
            index: o.index,
        });
        return Ok(events);
    }
    events.push(Event::CustodyDepositCredited {
        custodian,
        owner,
        asset: o.asset.clone(),
        amount: o.amount,
        tx_hash: tokens::hex(&o.tx_hash),
        index: o.index,
    });
    if owner == custodian {
        settle_expenses(state, custodian, &o.asset)?;
    }
    // A dollar stablecoin becomes USDT on arrival, as in the network vault.
    events.extend(mint_stable(
        state,
        custodian,
        owner,
        &o.asset,
        o.amount,
        &external_id,
    )?);
    Ok(events)
}

/// `ReleaseCustodyHold`: the custodian moves a held deposit to where it
/// may be spent: the owner's custody balance, or another account in its
/// custody (its own, a compliance account) when the owner is not to have
/// it. A dollar stablecoin becomes USDT as it is released, as it would
/// have on arrival.
fn release_hold(
    state: &mut State,
    signer: Address,
    tx_id: &[u8; 32],
    owner: Address,
    asset: &Asset,
    amount: Amount,
    to: Option<Address>,
) -> Result<Vec<Event>, VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let custodian = match state.custody.custodian_of.get(&owner) {
        Some(c) => *c,
        None => return Err(VmError::NotEnoughFunds),
    };
    if signer != custodian {
        return Err(VmError::Unauthorized);
    }
    let to = to.unwrap_or(owner);
    match state.custody.custodian_of.get(&to) {
        Some(c) if *c == custodian => {}
        Some(_) => {
            return Err(VmError::Invalid(
                "the recipient is in another client's custody".into(),
            ))
        }
        None if to == custodian => {}
        None => require_member(state, to, custodian).map_err(|_| {
            VmError::Invalid("the recipient is not an account of this client".into())
        })?,
    }
    if (state.ledger.balance(&hold_key(owner, asset)).max(0) as Amount) < amount {
        return Err(VmError::NotEnoughFunds);
    }
    let external_id = format!("custody:release:{}", tokens::hex(tx_id));
    state.ledger.post(
        &external_id,
        TxType::InternalTransferComplete,
        None,
        None,
        vec![
            Record::debit(hold_key(owner, asset), amount),
            Record::credit(custody_key(to, asset), amount),
        ],
    )?;
    if to != custodian {
        state.custody.custodian_of.entry(to).or_insert(custodian);
    }
    let mut events = vec![Event::CustodyHoldReleased {
        custodian,
        owner,
        to,
        asset: asset.clone(),
        amount,
    }];
    events.extend(mint_stable(
        state,
        custodian,
        to,
        asset,
        amount,
        &external_id,
    )?);
    Ok(events)
}

// ---------------- One USDT in custody (2026-11-21) ----------------
//
// A client's vault receives USDT on several networks, and its users hold
// one USDT. So a custody deposit of an asset in the stable basket is
// credited as the unified USDT, exactly as a deposit to the network vault
// is, with one difference: the backing never leaves the client. The
// deposited coins stay in the client's own vault, earmarked in
// `stable_backing`, and the USDT is issued against them from the client's
// own `custody_issuance` account. A withdrawal names the network to be
// paid on and is redeemed out of what the client's vault holds there.
//
// Invariant, per custodian: its custody USDT <= the sum of its backing
// converted to USDT units. Conversion floors, so rounding only ever leaves
// backing behind. The per-asset reserve check is unchanged, because the
// backing stays counted in that asset's liabilities: the coins are still
// owed, to the holders of the USDT.

/// Whether a custody balance of `asset` is held as the unified USDT.
fn unifies(state: &State, asset: &Asset) -> bool {
    state.stable_conversion.params.auto_convert == 1
        && state.stable.basket.get(asset).is_some_and(|e| e.enabled)
}

/// `custodian`'s backing, in USDT units.
pub fn stable_backing(state: &State, custodian: Address) -> Amount {
    let sd = state.tokens.decimals(&state.tokens.stable).unwrap_or(6);
    state
        .custody
        .stable_backing
        .iter()
        .filter(|((c, _), _)| *c == custodian)
        .map(|((_, a), v)| super::stable::convert(*v, state.tokens.decimals(a).unwrap_or(sd), sd))
        .fold(0, Amount::saturating_add)
}

/// Turn `amount` of `asset` on `owner`'s custody balance into USDT. Does
/// nothing when the asset is not unified or the amount is below one unit
/// of USDT.
pub fn mint_stable(
    state: &mut State,
    custodian: Address,
    owner: Address,
    asset: &Asset,
    amount: Amount,
    external_id: &str,
) -> Result<Vec<Event>, VmError> {
    if !unifies(state, asset) {
        return Ok(Vec::new());
    }
    let (rd, sd) = super::stable::decimals(state, asset)?;
    let minted = super::stable::convert(amount, rd, sd);
    if minted == 0 {
        return Ok(Vec::new());
    }
    let stable = state.tokens.stable.clone();
    state.ledger.post(
        &format!("{external_id}:unify"),
        TxType::StableMint,
        None,
        None,
        vec![
            Record::debit(custody_key(owner, asset), amount),
            Record::credit(stable_reserve_key(custodian, asset), amount),
        ],
    )?;
    state.ledger.post(
        &format!("{external_id}:unify:issue"),
        TxType::StableMint,
        None,
        None,
        vec![
            Record::debit(issuance_key(custodian, &stable), minted),
            Record::credit(custody_key(owner, &stable), minted),
        ],
    )?;
    let b = state
        .custody
        .stable_backing
        .entry((custodian, asset.clone()))
        .or_insert(0);
    *b = b.saturating_add(amount);
    add_liability(state, custodian, &stable, minted);
    Ok(vec![Event::CustodyStableMinted {
        custodian,
        owner,
        asset: asset.clone(),
        amount_in: amount,
        minted,
    }])
}

/// Turn `burn` USDT on `owner`'s custody balance into `asset` out of the
/// custodian's backing on that network. Returns the amount of `asset`.
fn redeem_stable(
    state: &mut State,
    custodian: Address,
    owner: Address,
    asset: &Asset,
    burn: Amount,
    external_id: &str,
) -> Result<(Amount, Vec<Event>), VmError> {
    if !state.stable.basket.contains_key(asset) {
        return Err(VmError::Invalid(format!("{asset} does not back USDT")));
    }
    let (rd, sd) = super::stable::decimals(state, asset)?;
    let out = super::stable::convert(burn, sd, rd);
    if out == 0 {
        return Err(VmError::Invalid("amount too small".into()));
    }
    let held = state
        .custody
        .stable_backing
        .get(&(custodian, asset.clone()))
        .copied()
        .unwrap_or(0);
    if held < out {
        return Err(VmError::Invalid(format!(
            "the client's vault holds {held} {asset} for USDT, not the {out} this needs; choose another network"
        )));
    }
    let stable = state.tokens.stable.clone();
    if balance(state, owner, &stable) < burn {
        return Err(VmError::NotEnoughFunds);
    }
    state.ledger.post(
        &format!("{external_id}:redeem:burn"),
        TxType::StableBurn,
        None,
        None,
        vec![
            Record::debit(custody_key(owner, &stable), burn),
            Record::credit(issuance_key(custodian, &stable), burn),
        ],
    )?;
    state.ledger.post(
        &format!("{external_id}:redeem"),
        TxType::StableBurn,
        None,
        None,
        vec![
            Record::debit(stable_reserve_key(custodian, asset), out),
            Record::credit(custody_key(owner, asset), out),
        ],
    )?;
    state
        .custody
        .stable_backing
        .insert((custodian, asset.clone()), held - out);
    sub_liability(state, custodian, &stable, burn);
    Ok((
        out,
        vec![Event::CustodyStableRedeemed {
            custodian,
            owner,
            asset: asset.clone(),
            burned: burn,
            amount_out: out,
        }],
    ))
}

/// `WithdrawCustodyStable`: redeem `amount` USDT into `via` and queue the
/// payout from the custodian's vault, in one action.
fn withdraw_stable(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    w: &keel_actions::WithdrawStable,
) -> Result<(Address, Vec<Event>), VmError> {
    if w.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let custodian = *state
        .custody
        .custodian_of
        .get(&signer)
        .ok_or(VmError::NotEnoughFunds)?;
    let chain = vaults::chain_of(state, &w.via)?;
    // Checked before anything is redeemed: a refused payout must not
    // leave the account holding the per-network asset.
    if !vaults::valid_destination(chain, &w.to) {
        return Err(VmError::Invalid("bad destination address".into()));
    }
    if state.custody.vault(chain, custodian).is_none() {
        return Err(VmError::NotFound(format!(
            "custody vault on {}",
            chain.as_str()
        )));
    }
    let stable = state.tokens.stable.clone();
    for a in [&stable, &w.via] {
        if state.custody.halted.contains(&(custodian, a.clone())) {
            return Err(VmError::Paused(format!(
                "custody outbounds for {a} (reserve below balances)"
            )));
        }
    }
    let group = format!("custody:stable:{}", tokens::hex(tx_id));
    let (out, mut events) = redeem_stable(state, custodian, signer, &w.via, w.amount, &group)?;
    let outbound_id = state.vaults.next_outbound_id;
    let (_, queued) = withdraw(
        state,
        ctx,
        signer,
        tx_id,
        &Withdraw {
            asset: w.via.clone(),
            to: w.to.clone(),
            amount: out,
        },
    )?;
    state.custody.stable_outbounds.insert(outbound_id);
    events.extend(queued);
    Ok((custodian, events))
}

fn expense_key(custodian: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(custodian, asset.clone(), "sendout_network_fee").expect("catalog type")
}

/// Network fees the vault paid while the client held no gas were booked
/// as the client's expense; its next top-up settles them from its own
/// custody balance so the reserve covers the balances again.
fn settle_expenses(state: &mut State, custodian: Address, asset: &Asset) -> Result<(), VmError> {
    let owed = (-state.ledger.balance(&expense_key(custodian, asset))).max(0) as Amount;
    let pay = owed.min(balance(state, custodian, asset));
    if pay == 0 {
        return Ok(());
    }
    state.ledger.post(
        &format!(
            "custody:expense:{}:{}:{}",
            custodian.to_hex(),
            asset,
            state.height
        ),
        TxType::SendoutComplete,
        None,
        None,
        vec![
            Record::debit(custody_key(custodian, asset), pay),
            Record::credit(expense_key(custodian, asset), pay),
        ],
    )?;
    sub_liability(state, custodian, asset, pay);
    Ok(())
}

fn withdraw(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    w: &Withdraw,
) -> Result<(Address, Vec<Event>), VmError> {
    if w.asset == state.tokens.stable {
        return Err(VmError::Invalid(
            "USDT leaves custody with WithdrawCustodyStable, which names the network to pay on"
                .into(),
        ));
    }
    let chain = vaults::chain_of(state, &w.asset)?;
    let custodian = *state
        .custody
        .custodian_of
        .get(&signer)
        .ok_or(VmError::NotEnoughFunds)?;
    if state.custody.vault(chain, custodian).is_none() {
        return Err(VmError::NotFound(format!(
            "custody vault on {}",
            chain.as_str()
        )));
    }
    if state.custody.halted.contains(&(custodian, w.asset.clone())) {
        return Err(VmError::Paused(format!(
            "custody outbounds for {} (reserve below balances)",
            w.asset
        )));
    }
    if w.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    if !vaults::valid_destination(chain, &w.to) {
        return Err(VmError::Invalid("bad destination address".into()));
    }
    let outbound_id = state.vaults.next_outbound_id;
    let group = format!("outbound:{outbound_id}");
    state.ledger.post(
        &format!("{group}:prepare:{}", tokens::hex(tx_id)),
        TxType::SendoutPrepare,
        Some(&group),
        None,
        vec![
            Record::debit(custody_key(signer, &w.asset), w.amount),
            Record::credit(escrow_key(signer, &w.asset), w.amount),
        ],
    )?;
    state.vaults.next_outbound_id += 1;
    state.vaults.outbounds.insert(
        outbound_id,
        Outbound {
            id: outbound_id,
            owner: signer,
            asset: w.asset.clone(),
            chain,
            to: w.to.clone(),
            amount: w.amount,
            // The client's vault pays its own network fees.
            fee_asset: vaults::native_asset(chain),
            fee_estimate: 0,
            status: OutboundStatus::Queued,
            batch_id: None,
            created_height: ctx.height,
            quorum: Quorum::default(),
            tx_hash: None,
            votes: Vec::new(),
            external_tx: None,
        },
    );
    state.custody.outbound_vault.insert(outbound_id, custodian);
    Ok((
        custodian,
        vec![Event::CustodyWithdrawalQueued {
            custodian,
            outbound_id,
            owner: signer,
            asset: w.asset.clone(),
            amount: w.amount,
            to: w.to.clone(),
        }],
    ))
}

/// The float swap (docs/models.md): `owner`'s balance changes which vault
/// backs it by swapping against the custodian's own balance on the other
/// side. Four legs in one posting, no external transaction, and both
/// reserve checks unchanged: the custody vault backs the same total (the
/// owner's custody balance became the custodian's, or the reverse) and so
/// does the network vault.
fn swap(
    state: &mut State,
    signer: Address,
    tx_id: &[u8; 32],
    s: &CustodySwap,
) -> Result<(Address, Vec<Event>), VmError> {
    if s.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    // The unified USDT lives on no single chain: it swaps like any asset,
    // against the custodian's USDT on either side.
    let is_stable = s.asset == state.tokens.stable;
    let chain = if is_stable {
        None
    } else {
        Some(vaults::chain_of(state, &s.asset)?)
    };
    // Whose balance moves, and which custodian's floats it moves against.
    let owner = s.on_behalf_of.unwrap_or(signer);
    let custodian = match state.custody.custodian_of.get(&owner) {
        Some(c) => *c,
        None => match s.to {
            SwapSide::Custody => clients::client_of(state, &owner)
                .ok_or_else(|| VmError::Invalid("account is in no client's custody".into()))?,
            SwapSide::Deposit => return Err(VmError::NotEnoughFunds),
        },
    };
    if owner != signer && signer != custodian {
        return Err(VmError::Unauthorized);
    }
    require_member(state, owner, custodian)?;
    let has_vault = match chain {
        Some(c) => state.custody.vault(c, custodian).is_some(),
        None => state.custody.vaults.keys().any(|(_, c)| *c == custodian),
    };
    if !has_vault {
        return Err(VmError::NotFound(format!(
            "custody vault of {} for {}",
            custodian.to_hex(),
            s.asset
        )));
    }
    if state.custody.halted.contains(&(custodian, s.asset.clone())) {
        return Err(VmError::Paused(format!(
            "custody of {} (reserve below balances)",
            s.asset
        )));
    }
    if state.vaults.halted.contains(&s.asset) {
        return Err(VmError::Paused(format!("network vault for {}", s.asset)));
    }
    state.custody.custodian_of.entry(owner).or_insert(custodian);
    let (from_owner, to_owner, from_cust, to_cust, side) = match s.to {
        SwapSide::Deposit => (
            custody_key(owner, &s.asset),
            tokens::deposit_key(owner, &s.asset),
            tokens::deposit_key(custodian, &s.asset),
            custody_key(custodian, &s.asset),
            "deposit",
        ),
        SwapSide::Custody => (
            tokens::deposit_key(owner, &s.asset),
            custody_key(owner, &s.asset),
            custody_key(custodian, &s.asset),
            tokens::deposit_key(custodian, &s.asset),
            "custody",
        ),
    };
    if state.ledger.balance(&from_owner) < s.amount as i128 {
        return Err(VmError::NotEnoughFunds);
    }
    if state.ledger.balance(&from_cust) < s.amount as i128 {
        return Err(VmError::Invalid(format!(
            "custodian float on the {side} side does not cover {} {}",
            s.amount, s.asset
        )));
    }
    state.ledger.post(
        &format!("custody:swap:{}", tokens::hex(tx_id)),
        TxType::VaultMigrate,
        None,
        None,
        vec![
            Record::debit(from_owner, s.amount),
            Record::credit(to_cust, s.amount),
            Record::debit(from_cust, s.amount),
            Record::credit(to_owner, s.amount),
        ],
    )?;
    Ok((
        custodian,
        vec![Event::CustodySwapped {
            custodian,
            owner,
            asset: s.asset.clone(),
            amount: s.amount,
            to: side.to_string(),
        }],
    ))
}

/// `TransferCustody`: custody balances move from one account to others of
/// the same custodian. The custodian's vault backs the same total before
/// and after, so its reserve check is untouched and nothing is sent on any
/// network. The owner signs, or the custodian signs for it; each recipient
/// is the custodian itself, an account already in its custody, or an
/// account it attested (which joins its custody by receiving). Every leg
/// is checked before anything is written and all of them are one ledger
/// posting, so the transfer happens whole or not at all.
fn transfer(
    state: &mut State,
    signer: Address,
    tx_id: &[u8; 32],
    t: &keel_actions::CustodyTransfer,
) -> Result<Vec<Event>, VmError> {
    if t.legs.is_empty() || t.legs.len() > keel_actions::MAX_CUSTODY_LEGS {
        return Err(VmError::Invalid(format!(
            "a custody transfer has 1 to {} legs",
            keel_actions::MAX_CUSTODY_LEGS
        )));
    }
    if t.memo
        .as_ref()
        .is_some_and(|m| m.len() > state.limits.max_memo_bytes as usize)
    {
        return Err(VmError::Invalid("memo too long".into()));
    }
    let owner = t.on_behalf_of.unwrap_or(signer);
    // A custodian's own balance is in its own custody.
    let custodian = match state.custody.custodian_of.get(&owner) {
        Some(c) => *c,
        None if state.custody.vaults.keys().any(|(_, c)| *c == owner) => owner,
        None => return Err(VmError::NotEnoughFunds),
    };
    if owner != signer && signer != custodian {
        return Err(VmError::Unauthorized);
    }
    // What each account ends up paying or receiving, per asset: the whole
    // set is one posting, so an account that is paid and pays in the same
    // transfer needs only the difference.
    let mut net: BTreeMap<(Address, Asset), i128> = BTreeMap::new();
    for leg in &t.legs {
        let from = leg.from.unwrap_or(owner);
        if leg.amount == 0 || leg.amount > i128::MAX as Amount {
            return Err(VmError::Invalid("amount must be > 0".into()));
        }
        if leg.to == from {
            return Err(VmError::Invalid("sender and recipient are the same".into()));
        }
        if leg.to.is_system() {
            return Err(VmError::Invalid(
                "cannot transfer to the system address".into(),
            ));
        }
        // A leg paid from another account: only the custodian of both
        // signs for it, and it stays inside that custodian's custody.
        if from != owner {
            if signer != custodian {
                return Err(VmError::Unauthorized);
            }
            let theirs = match state.custody.custodian_of.get(&from) {
                Some(c) => *c,
                None if from == custodian => custodian,
                None => return Err(VmError::NotEnoughFunds),
            };
            if theirs != custodian {
                return Err(VmError::Invalid(
                    "the sender is in another client's custody".into(),
                ));
            }
        }
        if leg.asset != state.tokens.stable {
            vaults::chain_of(state, &leg.asset)?;
        }
        match state.custody.custodian_of.get(&leg.to) {
            Some(c) if *c == custodian => {}
            Some(_) => {
                return Err(VmError::Invalid(
                    "the recipient is in another client's custody".into(),
                ))
            }
            None if leg.to == custodian => {}
            None => require_member(state, leg.to, custodian).map_err(|_| {
                VmError::Invalid("the recipient is not an account of this client".into())
            })?,
        }
        if state
            .custody
            .halted
            .contains(&(custodian, leg.asset.clone()))
        {
            return Err(VmError::Paused(format!(
                "custody of {} (reserve below balances)",
                leg.asset
            )));
        }
        let amount = leg.amount as i128;
        for (who, delta) in [(from, -amount), (leg.to, amount)] {
            let n = net.entry((who, leg.asset.clone())).or_insert(0);
            *n = n
                .checked_add(delta)
                .ok_or_else(|| VmError::Invalid("overflow".into()))?;
        }
    }
    for ((who, asset), delta) in &net {
        if *delta < 0 && (balance(state, *who, asset) as i128) < -*delta {
            return Err(VmError::NotEnoughFunds);
        }
    }
    // Posted as the net movement per account, so the ledger sees no
    // account dip below zero on the way: debits, then credits.
    let mut records = Vec::with_capacity(net.len());
    for ((who, asset), delta) in &net {
        if *delta < 0 {
            records.push(Record::debit(custody_key(*who, asset), (-*delta) as Amount));
        }
    }
    for ((who, asset), delta) in &net {
        if *delta > 0 {
            records.push(Record::credit(custody_key(*who, asset), *delta as Amount));
        }
    }
    if !records.is_empty() {
        state.ledger.post(
            &format!("custody:transfer:{}", tokens::hex(tx_id)),
            TxType::InternalTransferComplete,
            None,
            None,
            records,
        )?;
    }
    let mut events = Vec::with_capacity(t.legs.len());
    for leg in &t.legs {
        if leg.to != custodian {
            state
                .custody
                .custodian_of
                .entry(leg.to)
                .or_insert(custodian);
        }
        events.push(Event::CustodyTransferred {
            custodian,
            from: leg.from.unwrap_or(owner),
            to: leg.to,
            asset: leg.asset.clone(),
            amount: leg.amount,
            memo: t.memo.clone(),
        });
    }
    Ok(events)
}

/// Called by the vaults module once the observer quorum settled a custody
/// outbound: the amount leaves the custodian's reserve, the network fee is
/// paid from the custodian's own native custody balance (its gas tank)
/// or, failing that, booked as its expense.
pub(crate) fn settle_outbound(
    state: &mut State,
    ob: &Outbound,
    custodian: Address,
    fee_paid: Amount,
    success: bool,
    tx_hash: &[u8; 32],
    external_tx: Option<String>,
) -> Result<Vec<Event>, VmError> {
    let group = format!("outbound:{}", ob.id);
    let mut events = Vec::new();
    if success {
        state.ledger.post(
            &format!("{group}:complete"),
            TxType::SendoutComplete,
            Some(&group),
            None,
            vec![
                Record::debit(escrow_key(ob.owner, &ob.asset), ob.amount),
                Record::credit(reserve_key(custodian, &ob.asset), ob.amount),
            ],
        )?;
        sub_liability(state, custodian, &ob.asset, ob.amount);
        if fee_paid > 0 {
            charge_network_fee(state, custodian, &ob.fee_asset, fee_paid, &group)?;
            events.push(Event::CustodyNetworkFeePaid {
                custodian,
                outbound_id: ob.id,
                asset: ob.fee_asset.clone(),
                amount: fee_paid,
            });
        }
        let out = state.vaults.outbounds.get_mut(&ob.id).expect("exists");
        out.status = OutboundStatus::Confirmed;
        events.push(Event::OutboundConfirmed {
            outbound_id: ob.id,
            tx_hash: tokens::hex(tx_hash),
            external_tx,
            owner: ob.owner,
            custodian: Some(custodian),
        });
    } else {
        state.ledger.post(
            &format!("{group}:failed"),
            TxType::SendoutFailed,
            Some(&group),
            None,
            vec![
                Record::debit(escrow_key(ob.owner, &ob.asset), ob.amount),
                Record::credit(custody_key(ob.owner, &ob.asset), ob.amount),
            ],
        )?;
        let out = state.vaults.outbounds.get_mut(&ob.id).expect("exists");
        out.status = OutboundStatus::Failed;
        events.push(Event::OutboundFailed {
            outbound_id: ob.id,
            refunded: ob.amount,
            owner: ob.owner,
            custodian: Some(custodian),
        });
        // A failed payout of redeemed USDT goes back to being USDT.
        if state.custody.stable_outbounds.contains(&ob.id) {
            events.extend(mint_stable(
                state,
                custodian,
                ob.owner,
                &ob.asset,
                ob.amount,
                &format!("{group}:failed"),
            )?);
        }
    }
    state.custody.stable_outbounds.remove(&ob.id);
    Ok(events)
}

/// Batch queued custody outbounds per (chain, custodian) on the same
/// cadence as the network vault, then check every custody reserve.
pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = Vec::new();
    let interval = state.params.outbound_batch_interval_blocks.max(1);
    if ctx.height.is_multiple_of(interval) {
        let mut groups: BTreeMap<(Chain, Address), Vec<OutboundId>> = BTreeMap::new();
        for o in state.vaults.outbounds.values() {
            if o.status != OutboundStatus::Queued {
                continue;
            }
            let Some(c) = state.custody.outbound_vault.get(&o.id) else {
                continue;
            };
            if state.custody.halted.contains(&(*c, o.asset.clone())) {
                continue;
            }
            groups.entry((o.chain, *c)).or_default().push(o.id);
        }
        for ((chain, custodian), ids) in groups {
            let batch_id = state.vaults.next_batch_id;
            state.vaults.next_batch_id += 1;
            for id in &ids {
                let o = state.vaults.outbounds.get_mut(id).expect("exists");
                o.status = OutboundStatus::Batched;
                o.batch_id = Some(batch_id);
                events.push(Event::OutboundBatched {
                    outbound_id: *id,
                    chain: chain.as_str().into(),
                });
            }
            state.vaults.batches.insert(
                batch_id,
                vaults::Batch {
                    id: batch_id,
                    chain,
                    outbound_ids: ids,
                    created_height: ctx.height,
                },
            );
            state.custody.batch_vault.insert(batch_id, custodian);
            let gen = super::pools::batch_generation(state, chain, custodian);
            state.pools.batch_gen.insert(batch_id, gen);
        }
    }
    // Reserve check per custody vault and asset. A vault halts when its
    // reserve no longer covers the balances it backs and resumes on its
    // own once it does again.
    let rows: Vec<((Address, Asset), Amount)> = state
        .custody
        .liabilities
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let stable = state.tokens.stable.clone();
    for ((custodian, asset), liabilities) in rows {
        // The unified USDT is backed by the client's dollar stablecoins,
        // not by a vault balance of its own.
        let reserve = if asset == stable {
            stable_backing(state, custodian) as i128
        } else {
            state.custody.reserve(state, custodian, &asset)
        };
        let breached = reserve < liabilities as i128;
        let key = (custodian, asset.clone());
        if breached && !state.custody.halted.contains(&key) {
            state.custody.halted.insert(key);
            events.push(Event::CustodyReserveBreached {
                custodian,
                asset,
                reserve: reserve.max(0) as Amount,
                liabilities,
            });
        } else if !breached && state.custody.halted.contains(&key) {
            state.custody.halted.remove(&key);
        }
    }
    events
}

/// Sum of every custody reserve in `asset` (non-system `vault_asset`
/// accounts) and of every custody liability, so the network reserve check
/// can leave client vaults out.
pub fn totals(state: &State, asset: &Asset) -> (i128, Amount) {
    let reserves = state.ledger.sum_balances(asset, |k| {
        !k.is_system() && k == &reserve_key(k.owner, asset)
    });
    let liabilities = state
        .custody
        .liabilities
        .iter()
        .filter(|((_, a), _)| a == asset)
        .map(|(_, v)| *v)
        .fold(0, Amount::saturating_add);
    (reserves, liabilities)
}

/// Is `who` a bonded observer (for the RPC's readiness view).
pub fn is_observer(state: &State, who: &Address) -> bool {
    staking::is_observer(state, who)
}
