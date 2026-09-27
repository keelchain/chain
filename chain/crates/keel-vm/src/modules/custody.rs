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
    Action, Chain, CustodyVaultRegistration, DepositObservation, OutboundId, Withdraw,
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
        Action::RequestCustodyAddress { chain, custodian } => {
            let mut events = request_address(state, signer, *chain, *custodian)?;
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
    if r.public_key.len() != 33 {
        return Err(VmError::Invalid("bad vault public key length".into()));
    }
    if r.chain_code.is_none() {
        return Err(VmError::Invalid(
            "a custody vault needs a chain code".into(),
        ));
    }
    let url_ok = (r.signer_url.starts_with("http://") || r.signer_url.starts_with("https://"))
        && r.signer_url.len() <= 256
        && r.signer_url.chars().all(|c| c.is_ascii_graphic());
    if !url_ok {
        return Err(VmError::Invalid("signer_url must be an http(s) URL".into()));
    }
    if let Some(v) = state.custody.vault(r.chain, signer) {
        if r.epoch <= v.epoch {
            return Err(VmError::Invalid(format!(
                "vault epoch must exceed {}",
                v.epoch
            )));
        }
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
    state.ledger.post(
        &external_id,
        TxType::VaultDeposit,
        None,
        None,
        vec![
            Record::debit(reserve_key(custodian, &o.asset), o.amount),
            Record::credit(custody_key(owner, &o.asset), o.amount),
        ],
    )?;
    state.custody.credited.insert(key);
    add_liability(state, custodian, &o.asset, o.amount);
    events.push(Event::CustodyDepositCredited {
        custodian,
        owner,
        asset: o.asset.clone(),
        amount: o.amount,
    });
    if owner == custodian {
        settle_expenses(state, custodian, &o.asset)?;
    }
    Ok(events)
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
    let chain = vaults::chain_of(state, &w.asset)?;
    let custodian = *state
        .custody
        .custodian_of
        .get(&signer)
        .ok_or_else(|| VmError::Invalid("account holds no custody balance".into()))?;
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
            let native = &ob.fee_asset;
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
                Some(&group),
                None,
                vec![
                    Record::debit(debit, fee_paid),
                    Record::credit(reserve_key(custodian, native), fee_paid),
                ],
            )?;
            if from_custody {
                sub_liability(state, custodian, native, fee_paid);
            }
        }
        let out = state.vaults.outbounds.get_mut(&ob.id).expect("exists");
        out.status = OutboundStatus::Confirmed;
        events.push(Event::OutboundConfirmed {
            outbound_id: ob.id,
            tx_hash: tokens::hex(tx_hash),
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
        });
    }
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
    for ((custodian, asset), liabilities) in rows {
        let reserve = state.custody.reserve(state, custodian, &asset);
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
