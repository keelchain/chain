//! Cross-chain vaults (docs/plan.md §4). Deposits: observers attest
//! to an external transfer landing on a chain-derived deposit address;
//! where a light client exists (BTC, ETH) the proof is verified in-VM and
//! the quorum is required on top of it. Withdrawals: funds move to
//! `sendout_escrow`, are batched per chain, signed and broadcast off-chain
//! by the observer-signers, and leave the reserves once the outbound is
//! observed back in with quorum.
//!
//! Money legs:
//!   VaultDeposit      system vault_asset -> owner deposit (or screening_hold)
//!   ScreeningRelease  owner screening_hold -> owner deposit
//!   SendoutPrepare    owner deposit -> owner sendout_escrow (amount + fee estimate)
//!   fee split         owner deposit -> treasury/validators/burn (flat fee)
//!   SendoutComplete   owner sendout_escrow -> system vault_asset
//!   SendoutFailed     owner sendout_escrow -> owner deposit
//!
//! Network fee pass-through: the median of the observers' `ReportNetworkFee`
//! rates times a per-chain size constant is locked at withdrawal time in the
//! chain's native asset when the user holds it, otherwise its USD equivalent
//! in the withdrawn asset (or nothing when no price exists yet). The actual
//! fee paid is reconciled on confirmation; overpayment is refunded.

use crate::{
    context::BlockContext,
    modules::{fees, staking, tokens},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{
    Action, Chain, DepositObservation, OutboundId, OutboundObservation, Proof, VaultRegistration,
    Withdraw,
};
use keel_attest::{Attestation, Quorum};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{mul_div_floor, pow10, Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::tokens::{deposit_key, system_key, AssetKind};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Vault {
    pub chain: Chain,
    pub epoch: u64,
    pub public_key: Vec<u8>,
    pub chain_code: Option<[u8; 32]>,
    pub signers: Vec<Address>,
    pub threshold: u32,
    pub registered_height: u64,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub enum DepositStatus {
    Pending,
    Credited,
    Held,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct PendingDeposit {
    pub attestation: Attestation,
    pub quorum: Quorum,
    pub first_height: u64,
    pub last_height: u64,
    pub status: DepositStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct HeldDeposit {
    pub owner: Address,
    pub asset: Asset,
    pub amount: Amount,
    pub release_height: u64,
    pub external_id: String,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub enum OutboundStatus {
    Queued,
    Batched,
    Confirmed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Outbound {
    pub id: OutboundId,
    pub owner: Address,
    pub asset: Asset,
    pub chain: Chain,
    pub to: String,
    pub amount: Amount,
    /// Native asset of the chain and the amount locked for network fees.
    pub fee_asset: Asset,
    pub fee_estimate: Amount,
    pub status: OutboundStatus,
    pub batch_id: Option<u64>,
    pub created_height: u64,
    pub quorum: Quorum,
    pub tx_hash: Option<[u8; 32]>,
    /// (observer, fee_paid, success) per vote; the quorum settles on the
    /// median fee and the majority verdict, not on the closing vote.
    #[serde(default)]
    pub votes: Vec<(Address, Amount, bool)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Batch {
    pub id: u64,
    pub chain: Chain,
    pub outbound_ids: Vec<OutboundId>,
    pub created_height: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct VaultsState {
    /// (chain, epoch) -> vault. The highest epoch per chain is active.
    pub vaults: BTreeMap<(Chain, u64), Vault>,
    pub next_deposit_index: BTreeMap<Chain, u64>,
    pub deposit_owner: BTreeMap<(Chain, u64), Address>,
    pub deposit_index_of: BTreeMap<(Address, Chain), u64>,
    /// attestation digest -> pending deposit
    pub pending: BTreeMap<[u8; 32], PendingDeposit>,
    /// (chain, tx_hash, index) already credited or held.
    pub credited: BTreeSet<(Chain, [u8; 32], u32)>,
    pub held: Vec<HeldDeposit>,
    pub btc_chain: Option<keel_lc_btc::HeaderChain>,
    pub eth_sync: Option<keel_lc_eth::SyncCommitteeState>,
    /// ERC-20 contract per Ethereum vault asset (20 bytes).
    pub token_contracts: BTreeMap<Asset, Vec<u8>>,
    pub outbounds: BTreeMap<OutboundId, Outbound>,
    pub next_outbound_id: u64,
    pub batches: BTreeMap<u64, Batch>,
    pub next_batch_id: u64,
    pub halted: BTreeSet<Asset>,
    /// chain -> observer -> latest reported fee rate.
    pub fee_reports: BTreeMap<Chain, BTreeMap<Address, u64>>,
    /// Successful credits per chain, for daily caps: (day, usd micro).
    pub credited_today: BTreeMap<Chain, (u64, Amount)>,
}

impl VaultsState {
    pub fn active_vault(&self, chain: Chain) -> Option<&Vault> {
        self.vaults
            .range((chain, 0)..=(chain, u64::MAX))
            .next_back()
            .map(|(_, v)| v)
    }

    pub fn deposit_index(&self, owner: Address, chain: Chain) -> Option<u64> {
        self.deposit_index_of.get(&(owner, chain)).copied()
    }

    /// Median of the observers' latest fee reports for `chain`.
    pub fn fee_rate(&self, chain: Chain) -> u64 {
        let Some(reports) = self.fee_reports.get(&chain) else {
            return 0;
        };
        let mut v: Vec<u64> = reports.values().copied().collect();
        if v.is_empty() {
            return 0;
        }
        v.sort_unstable();
        v[v.len() / 2]
    }

    pub fn queued(&self, chain: Chain) -> Vec<&Outbound> {
        self.outbounds
            .values()
            .filter(|o| o.chain == chain && o.status == OutboundStatus::Queued)
            .collect()
    }
}

/// Native asset of a chain (fees are paid in it).
pub fn native_asset(chain: Chain) -> Asset {
    match chain {
        Chain::Bitcoin => Asset::vault("BTC", "BTC"),
        Chain::Ethereum => Asset::vault("ETH", "ETH"),
        Chain::Tron => Asset::vault("TRON", "TRX"),
    }
}

/// Fee-rate unit multiplier per chain: sat/vB × vbytes, wei/gas × gas,
/// sun (flat), lamports (flat).
pub fn fee_size(chain: Chain, is_native: bool) -> u64 {
    match chain {
        Chain::Bitcoin => 200,
        Chain::Ethereum => {
            if is_native {
                21_000
            } else {
                65_000
            }
        }
        Chain::Tron => 1,
    }
}

pub fn confirmations(state: &State, chain: Chain) -> u32 {
    match chain {
        Chain::Bitcoin => state.params.confirmations_btc,
        Chain::Ethereum => state.params.confirmations_eth,
        Chain::Tron => state.params.confirmations_tron,
    }
}

fn chain_of(state: &State, asset: &Asset) -> Result<Chain, VmError> {
    match state.tokens.kind(asset) {
        Some(AssetKind::Vault { chain }) => Ok(*chain),
        Some(_) => Err(VmError::Invalid(format!("{asset} is not a vault asset"))),
        None => Err(VmError::NotFound(format!("asset {asset}"))),
    }
}

/// Shape check of a destination address per chain. The observers' tx
/// builders (`keel-chains`) do the full decode; this stops obvious junk
/// from locking funds.
pub fn valid_destination(chain: Chain, to: &str) -> bool {
    let b58 = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() && !b"0OIl".contains(&c))
    };
    match chain {
        Chain::Bitcoin => {
            let lower = to.to_ascii_lowercase();
            let bech = (lower.starts_with("bc1")
                || lower.starts_with("tb1")
                || lower.starts_with("bcrt1"))
                && lower.len() >= 14
                && lower.len() <= 74
                && lower
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
            let legacy = (to.starts_with('1')
                || to.starts_with('3')
                || to.starts_with('m')
                || to.starts_with('n')
                || to.starts_with('2'))
                && (26..=35).contains(&to.len())
                && b58(to);
            bech || legacy
        }
        Chain::Ethereum => {
            to.len() == 42 && to.starts_with("0x") && to[2..].bytes().all(|c| c.is_ascii_hexdigit())
        }
        Chain::Tron => to.len() == 34 && to.starts_with('T') && b58(to),
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
        Action::RequestDepositAddress { chain } => request_address(state, signer, *chain),
        Action::ObserveDeposit(o) => observe_deposit(state, ctx, signer, o),
        Action::ObserveOutbound(o) => observe_outbound(state, ctx, signer, o),
        Action::ReportNetworkFee { chain, fee_rate } => {
            report_fee(state, signer, *chain, *fee_rate)
        }
        Action::Withdraw(w) => withdraw(state, ctx, signer, tx_id, w),
        Action::RegisterVault(r) => register_vault(state, ctx, signer, r),
        _ => Err(VmError::Invalid("not a vaults action".into())),
    }
}

fn request_address(
    state: &mut State,
    signer: Address,
    chain: Chain,
) -> Result<Vec<Event>, VmError> {
    if let Some(index) = state.vaults.deposit_index(signer, chain) {
        return Ok(vec![Event::DepositAddressAssigned {
            owner: signer,
            chain: chain.as_str().into(),
            index,
        }]);
    }
    let next = state.vaults.next_deposit_index.entry(chain).or_insert(1);
    let index = *next;
    *next += 1;
    state.vaults.deposit_owner.insert((chain, index), signer);
    state.vaults.deposit_index_of.insert((signer, chain), index);
    Ok(vec![Event::DepositAddressAssigned {
        owner: signer,
        chain: chain.as_str().into(),
        index,
    }])
}

fn require_observer(state: &State, signer: &Address) -> Result<(Vec<Address>, u32), VmError> {
    if !staking::is_observer(state, signer) {
        return Err(VmError::Unauthorized);
    }
    Ok(staking::observers(state))
}

fn verify_proof(state: &mut State, o: &DepositObservation) -> Result<(), VmError> {
    let required = confirmations(state, o.chain);
    match (&o.proof, o.chain) {
        // `o.index` is the OUTPUT index (vout), so one transaction paying
        // several vault addresses credits each output once; the proof's
        // `tx_index` is the transaction's position in its block and only
        // matters to the merkle check.
        (
            Proof::Bitcoin {
                headers,
                merkle_proof,
                tx_index,
            },
            Chain::Bitcoin,
        ) => {
            let res = match state.vaults.btc_chain.as_mut() {
                Some(chain) => {
                    chain.verify_deposit(headers, merkle_proof, *tx_index, o.tx_hash, required)
                }
                None => keel_lc_btc::verify_deposit(
                    headers,
                    merkle_proof,
                    *tx_index,
                    o.tx_hash,
                    required,
                    bitcoin_network(),
                ),
            };
            res.map(|_| ())
                .map_err(|e| VmError::Invalid(format!("btc proof: {e}")))
        }
        (Proof::Ethereum { proof }, Chain::Ethereum) => {
            let decoded = keel_lc_eth::EthDepositProof::try_from_slice(proof)
                .map_err(|_| VmError::Invalid("eth proof does not decode".into()))?;
            let contract = state.vaults.token_contracts.get(&o.asset).ok_or_else(|| {
                VmError::Invalid(format!("no token contract registered for {}", o.asset))
            })?;
            let token: [u8; 20] = contract
                .as_slice()
                .try_into()
                .map_err(|_| VmError::Invalid("bad token contract".into()))?;
            let sync = state
                .vaults
                .eth_sync
                .as_mut()
                .ok_or_else(|| VmError::Invalid("no eth checkpoint".into()))?;
            let v = sync
                .verify_deposit(&decoded, token)
                .map_err(|e| VmError::Invalid(format!("eth proof: {e}")))?;
            if v.amount != o.amount || decoded.log_index != o.index {
                return Err(VmError::Invalid(
                    "eth proof does not match the observation".into(),
                ));
            }
            Ok(())
        }
        (Proof::None, chain) if !chain.has_light_client() => Ok(()),
        (Proof::None, _) => Err(VmError::Invalid("light-client proof required".into())),
        _ => Err(VmError::Invalid("proof type does not match chain".into())),
    }
}

fn bitcoin_network() -> bitcoin_net::Network {
    bitcoin_net::Network::Bitcoin
}

mod bitcoin_net {
    pub use keel_lc_btc::Network;
}

fn observe_deposit(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    o: &DepositObservation,
) -> Result<Vec<Event>, VmError> {
    let (observers, threshold) = require_observer(state, &signer)?;
    if chain_of(state, &o.asset)? != o.chain {
        return Err(VmError::Invalid(
            "asset is not custodied on that chain".into(),
        ));
    }
    // Index 0 is the vault's own address: Lightning pool sweeps land there.
    let owner = if o.deposit_index == 0 {
        Address::SYSTEM
    } else {
        *state
            .vaults
            .deposit_owner
            .get(&(o.chain, o.deposit_index))
            .ok_or_else(|| {
                VmError::NotFound(format!(
                    "deposit index {} on {}",
                    o.deposit_index,
                    o.chain.as_str()
                ))
            })?
    };
    if o.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let key = (o.chain, o.tx_hash, o.index);
    if state.vaults.credited.contains(&key) {
        return Err(VmError::Invalid("deposit already credited".into()));
    }
    if owner.is_system() && !state.lightning.sweeps.contains_key(&o.tx_hash) {
        // Checked before the vote is recorded so a premature report does
        // not burn the observer's vote.
        return Err(VmError::Invalid(
            "deposit to the vault's own address was not announced as a Lightning sweep".into(),
        ));
    }
    let depth = o.tip_height.saturating_sub(o.external_height) as u32;
    let required = confirmations(state, o.chain);
    if depth < required {
        return Err(VmError::Invalid(format!(
            "need {required} confirmations, have {depth}"
        )));
    }
    verify_proof(state, o)?;

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
        .vaults
        .pending
        .entry(digest)
        .or_insert_with(|| PendingDeposit {
            attestation: attestation.clone(),
            quorum: Quorum::default(),
            first_height: ctx.height,
            last_height: ctx.height,
            status: DepositStatus::Pending,
        });
    if entry.status != DepositStatus::Pending {
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

    // Quorum: credit. A deposit to the vault's own address is a Lightning
    // pool sweep coming back (2026-09-10).
    if owner.is_system() {
        let external_id = format!(
            "vault:sweep:{}:{}:{}",
            o.chain.as_str(),
            tokens::hex(&o.tx_hash),
            o.index
        );
        let mut ev = super::lightning::on_sweep_deposit(state, &o.tx_hash, o.amount, &external_id)?;
        let pending = state
            .vaults
            .pending
            .get_mut(&digest)
            .expect("inserted above");
        pending.status = DepositStatus::Credited;
        state.vaults.credited.insert(key);
        events.append(&mut ev);
        return Ok(events);
    }
    let usd = tokens::usd_value(state, &o.asset, o.amount).unwrap_or(0);
    let large = state.params.large_deposit_usd_micro > 0
        && usd >= state.params.large_deposit_usd_micro as Amount;
    let external_id = format!(
        "vault:deposit:{}:{}:{}",
        o.chain.as_str(),
        tokens::hex(&o.tx_hash),
        o.index
    );
    let target = if large { "screening_hold" } else { "deposit" };
    let to = AccountKey::new(owner, o.asset.clone(), target).expect("catalog type");
    state.ledger.post(
        &external_id,
        TxType::VaultDeposit,
        Some(&external_id),
        None,
        vec![
            Record::debit(system_key(&o.asset, "vault_asset"), o.amount),
            Record::credit(to, o.amount),
        ],
    )?;
    let pending = state
        .vaults
        .pending
        .get_mut(&digest)
        .expect("inserted above");
    pending.status = if large {
        DepositStatus::Held
    } else {
        DepositStatus::Credited
    };
    state.vaults.credited.insert(key);
    if large {
        let release_height = ctx.height + state.params.large_deposit_delay_blocks;
        state.vaults.held.push(HeldDeposit {
            owner,
            asset: o.asset.clone(),
            amount: o.amount,
            release_height,
            external_id,
        });
        events.push(Event::DepositHeld {
            owner,
            asset: o.asset.clone(),
            amount: o.amount,
            release_height,
        });
    } else {
        events.push(Event::DepositCredited {
            owner,
            asset: o.asset.clone(),
            amount: o.amount,
        });
    }
    Ok(events)
}

fn report_fee(
    state: &mut State,
    signer: Address,
    chain: Chain,
    fee_rate: u64,
) -> Result<Vec<Event>, VmError> {
    require_observer(state, &signer)?;
    state
        .vaults
        .fee_reports
        .entry(chain)
        .or_default()
        .insert(signer, fee_rate);
    Ok(vec![Event::NetworkFeeReported {
        chain: chain.as_str().into(),
        observer: signer,
        fee_rate,
    }])
}

fn register_vault(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    r: &VaultRegistration,
) -> Result<Vec<Event>, VmError> {
    require_observer(state, &signer)?;
    // Every supported chain signs with secp256k1: a compressed 33-byte key.
    if r.public_key.len() != 33 {
        return Err(VmError::Invalid("bad vault public key length".into()));
    }
    if r.signers.is_empty() || r.threshold == 0 || r.threshold as usize > r.signers.len() {
        return Err(VmError::Invalid("bad signer set or threshold".into()));
    }
    if let Some(active) = state.vaults.active_vault(r.chain) {
        if r.epoch <= active.epoch {
            return Err(VmError::Invalid(format!(
                "vault epoch must exceed {}",
                active.epoch
            )));
        }
    }
    state.vaults.vaults.insert(
        (r.chain, r.epoch),
        Vault {
            chain: r.chain,
            epoch: r.epoch,
            public_key: r.public_key.clone(),
            chain_code: r.chain_code,
            signers: r.signers.clone(),
            threshold: r.threshold,
            registered_height: ctx.height,
        },
    );
    Ok(vec![Event::VaultRegistered {
        chain: r.chain.as_str().into(),
        epoch: r.epoch,
    }])
}

/// Units of `asset` worth `usd_micro`, floored; `None` without a price.
fn asset_units_for_usd(state: &State, asset: &Asset, usd_micro: Amount) -> Option<Amount> {
    let decimals = state.tokens.decimals(asset)?;
    let unit = pow10(decimals);
    let price = tokens::usd_value(state, asset, unit)?;
    if price == 0 {
        return None;
    }
    mul_div_floor(usd_micro, unit, price)
}

fn withdraw(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    w: &Withdraw,
) -> Result<Vec<Event>, VmError> {
    let chain = chain_of(state, &w.asset)?;
    // A BOLT11 destination is a Lightning payout (2026-09-10).
    if chain == Chain::Bitcoin && keel_ln::looks_like_invoice(&w.to) {
        return super::lightning::queue_payout(state, ctx, signer, tx_id, w);
    }
    if state.vaults.halted.contains(&w.asset) {
        return Err(VmError::Paused(format!("outbounds for {}", w.asset)));
    }
    if w.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    if !valid_destination(chain, &w.to) {
        return Err(VmError::Invalid("bad destination address".into()));
    }
    if state.vaults.active_vault(chain).is_none() {
        return Err(VmError::Invalid(format!(
            "no vault registered for {}",
            chain.as_str()
        )));
    }
    let id = tokens::hex(tx_id);
    let native = native_asset(chain);
    let is_native = native == w.asset;

    // Flat fee in the withdrawn asset (0 when no price exists yet).
    let flat = asset_units_for_usd(
        state,
        &w.asset,
        state.params.withdraw_flat_fee_usd_micro as Amount,
    )
    .unwrap_or(0);
    if flat >= w.amount {
        return Err(VmError::Invalid(
            "amount does not cover the withdrawal fee".into(),
        ));
    }
    // Network fee estimate in the chain's native asset.
    let rate = state.vaults.fee_rate(chain);
    let native_fee = (rate as Amount).saturating_mul(fee_size(chain, is_native) as Amount);
    let (fee_asset, fee_estimate) = if native_fee == 0 {
        (native.clone(), 0)
    } else if is_native || tokens::balance(state, signer, &native) >= native_fee {
        (native.clone(), native_fee)
    } else {
        // Charge the USD equivalent in the withdrawn asset.
        let usd = tokens::usd_value(state, &native, native_fee).unwrap_or(0);
        let equiv = asset_units_for_usd(state, &w.asset, usd).unwrap_or(0);
        (w.asset.clone(), equiv)
    };

    let outbound_id = state.vaults.next_outbound_id;
    let group = format!("outbound:{outbound_id}");
    // Lock the amount (and the fee when it is the same asset) in escrow.
    let mut lock = w.amount;
    if fee_asset == w.asset {
        lock = lock.saturating_add(fee_estimate);
    }
    state.ledger.post(
        &format!("{group}:prepare:{id}"),
        TxType::SendoutPrepare,
        Some(&group),
        None,
        vec![
            Record::debit(deposit_key(signer, &w.asset), lock),
            Record::credit(escrow_key(signer, &w.asset), lock),
        ],
    )?;
    if fee_asset != w.asset && fee_estimate > 0 {
        state.ledger.post(
            &format!("{group}:fee-lock:{id}"),
            TxType::SendoutPrepare,
            Some(&group),
            None,
            vec![
                Record::debit(deposit_key(signer, &fee_asset), fee_estimate),
                Record::credit(escrow_key(signer, &fee_asset), fee_estimate),
            ],
        )?;
    }
    if flat > 0 {
        fees::collect(
            state,
            &format!("{group}:flat:{id}"),
            TxType::SendoutComplete,
            Some(&group),
            deposit_key(signer, &w.asset),
            &w.asset,
            flat,
        )?;
    }

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
            fee_asset,
            fee_estimate,
            status: OutboundStatus::Queued,
            batch_id: None,
            created_height: ctx.height,
            quorum: Quorum::default(),
            tx_hash: None,
            votes: Vec::new(),
        },
    );
    Ok(vec![Event::WithdrawalQueued {
        outbound_id,
        owner: signer,
        asset: w.asset.clone(),
        amount: w.amount,
        to: w.to.clone(),
    }])
}

fn escrow_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "sendout_escrow").expect("catalog type")
}

fn observe_outbound(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    o: &OutboundObservation,
) -> Result<Vec<Event>, VmError> {
    let (observers, threshold) = require_observer(state, &signer)?;
    let ob = state
        .vaults
        .outbounds
        .get_mut(&o.outbound_id)
        .ok_or_else(|| VmError::NotFound(format!("outbound {}", o.outbound_id)))?;
    if ob.status != OutboundStatus::Batched {
        return Err(VmError::Invalid(
            "outbound is not awaiting confirmation".into(),
        ));
    }
    if !ob.quorum.vote(signer) {
        return Err(VmError::Invalid("observer already voted".into()));
    }
    ob.votes.push((signer, o.fee_paid, o.success));
    if ob.tx_hash.is_none() {
        ob.tx_hash = Some(o.tx_hash);
    }
    if !ob.quorum.reached(&observers, threshold) {
        return Ok(Vec::new());
    }
    let ob = ob.clone();
    // Settle on what the quorum agrees on: the median reported fee and the
    // majority verdict. A single observer's figure (say, one that only saw
    // the payment on chain and reported the estimate) cannot decide either.
    let mut fees: Vec<Amount> = ob.votes.iter().map(|(_, f, _)| *f).collect();
    fees.sort_unstable();
    let fee_paid = fees.get(fees.len() / 2).copied().unwrap_or(o.fee_paid);
    let successes = ob.votes.iter().filter(|(_, _, ok)| *ok).count();
    let success = successes * 2 >= ob.votes.len();
    let group = format!("outbound:{}", ob.id);
    let mut events = Vec::new();
    if state.lightning.fundings.contains_key(&ob.id) {
        // A Lightning pool funding: reserves move vault → pool (2026-09-10).
        super::lightning::on_funding_settled(state, ob.id, success, fee_paid.min(ob.fee_estimate))?;
        let out = state.vaults.outbounds.get_mut(&ob.id).expect("exists");
        out.status = if success {
            OutboundStatus::Confirmed
        } else {
            OutboundStatus::Failed
        };
        events.push(if success {
            Event::OutboundConfirmed {
                outbound_id: ob.id,
                tx_hash: tokens::hex(&o.tx_hash),
            }
        } else {
            Event::OutboundFailed {
                outbound_id: ob.id,
                refunded: 0,
            }
        });
        return Ok(events);
    }
    if success {
        // Amount leaves the reserves.
        state.ledger.post(
            &format!("{group}:complete"),
            TxType::SendoutComplete,
            Some(&group),
            None,
            vec![
                Record::debit(escrow_key(ob.owner, &ob.asset), ob.amount),
                Record::credit(system_key(&ob.asset, "vault_asset"), ob.amount),
            ],
        )?;
        // Network fee: what was paid leaves the reserves, the rest is refunded.
        if ob.fee_estimate > 0 {
            let paid = fee_paid.min(ob.fee_estimate);
            let refund = ob.fee_estimate - paid;
            let mut records = Vec::new();
            if paid > 0 {
                records.push(Record::debit(escrow_key(ob.owner, &ob.fee_asset), paid));
                records.push(Record::credit(
                    system_key(&ob.fee_asset, "vault_asset"),
                    paid,
                ));
            }
            if refund > 0 {
                records.push(Record::debit(escrow_key(ob.owner, &ob.fee_asset), refund));
                records.push(Record::credit(deposit_key(ob.owner, &ob.fee_asset), refund));
            }
            if !records.is_empty() {
                state.ledger.post(
                    &format!("{group}:fee"),
                    TxType::SendoutComplete,
                    Some(&group),
                    None,
                    records,
                )?;
            }
        }
        let out = state.vaults.outbounds.get_mut(&ob.id).expect("exists");
        out.status = OutboundStatus::Confirmed;
        events.push(Event::OutboundConfirmed {
            outbound_id: ob.id,
            tx_hash: tokens::hex(&o.tx_hash),
        });
    } else {
        // Failed on chain: everything back to the owner.
        let mut records = vec![
            Record::debit(escrow_key(ob.owner, &ob.asset), ob.amount),
            Record::credit(deposit_key(ob.owner, &ob.asset), ob.amount),
        ];
        let refunded = ob.amount;
        if ob.fee_estimate > 0 {
            if ob.fee_asset == ob.asset {
                records[0].amount += ob.fee_estimate;
                records[1].amount += ob.fee_estimate;
            } else {
                records.push(Record::debit(
                    escrow_key(ob.owner, &ob.fee_asset),
                    ob.fee_estimate,
                ));
                records.push(Record::credit(
                    deposit_key(ob.owner, &ob.fee_asset),
                    ob.fee_estimate,
                ));
            }
        }
        state.ledger.post(
            &format!("{group}:failed"),
            TxType::SendoutFailed,
            Some(&group),
            None,
            records,
        )?;
        let out = state.vaults.outbounds.get_mut(&ob.id).expect("exists");
        out.status = OutboundStatus::Failed;
        events.push(Event::OutboundFailed {
            outbound_id: ob.id,
            refunded,
        });
    }
    let _ = ctx;
    Ok(events)
}

pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = Vec::new();
    // Release held deposits.
    let due: Vec<HeldDeposit> = state
        .vaults
        .held
        .iter()
        .filter(|h| h.release_height <= ctx.height)
        .cloned()
        .collect();
    if !due.is_empty() {
        state.vaults.held.retain(|h| h.release_height > ctx.height);
        for h in due {
            let hold =
                AccountKey::new(h.owner, h.asset.clone(), "screening_hold").expect("catalog type");
            if state
                .ledger
                .post(
                    &format!("{}:release", h.external_id),
                    TxType::ScreeningRelease,
                    Some(&h.external_id),
                    None,
                    vec![
                        Record::debit(hold, h.amount),
                        Record::credit(deposit_key(h.owner, &h.asset), h.amount),
                    ],
                )
                .is_ok()
            {
                events.push(Event::DepositReleased {
                    owner: h.owner,
                    asset: h.asset,
                    amount: h.amount,
                });
            }
        }
    }
    // Batch queued outbounds per chain.
    let interval = state.params.outbound_batch_interval_blocks.max(1);
    if ctx.height.is_multiple_of(interval) {
        for chain in [Chain::Bitcoin, Chain::Ethereum, Chain::Tron] {
            let ids: Vec<OutboundId> = state
                .vaults
                .outbounds
                .values()
                .filter(|o| {
                    o.chain == chain
                        && o.status == OutboundStatus::Queued
                        && !state.vaults.halted.contains(&o.asset)
                })
                .map(|o| o.id)
                .collect();
            if ids.is_empty() {
                continue;
            }
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
                Batch {
                    id: batch_id,
                    chain,
                    outbound_ids: ids,
                    created_height: ctx.height,
                },
            );
        }
    }
    events
}

/// Invariant hook: stop paying out `asset` until governance resumes it.
/// A `ParamChange` cannot clear this; only [`resume_outbounds`] can.
pub fn halt_outbounds(state: &mut State, asset: &Asset) {
    state.vaults.halted.insert(asset.clone());
}

/// Governance hook: allow outbounds of `asset` again.
pub fn resume_outbounds(state: &mut State, asset: &Asset) {
    state.vaults.halted.remove(asset);
}

/// Governance/genesis hooks for light-client checkpoints and token contracts.
/// Build and install a Bitcoin header chain from a governance/genesis
/// checkpoint. Refused when the header does not decode or fails its PoW.
pub fn apply_btc_checkpoint(
    state: &mut State,
    cp: &keel_actions::BtcCheckpoint,
) -> Result<(), VmError> {
    let network = match cp.network {
        0 => keel_lc_btc::Network::Bitcoin,
        1 => keel_lc_btc::Network::Testnet,
        2 => keel_lc_btc::Network::Signet,
        3 => keel_lc_btc::Network::Regtest,
        _ => return Err(VmError::Invalid("unknown bitcoin network code".into())),
    };
    let chain = keel_lc_btc::HeaderChain::from_checkpoint(
        network,
        cp.height,
        &cp.header,
        cp.period_start_time,
    )
    .map_err(|e| VmError::Invalid(format!("bad btc checkpoint: {e}")))?;
    set_btc_checkpoint(state, chain);
    Ok(())
}

pub fn apply_eth_checkpoint(state: &mut State, cp: &keel_actions::EthCheckpoint) {
    let sync = keel_lc_eth::SyncCommitteeState::bootstrap(
        cp.period,
        cp.committee_root,
        cp.next_committee_root,
        cp.genesis_validators_root,
        cp.fork_version,
        cp.committee_size,
    );
    set_eth_checkpoint(state, sync);
}

pub fn set_btc_checkpoint(state: &mut State, chain: keel_lc_btc::HeaderChain) {
    state.vaults.btc_chain = Some(chain);
}

pub fn set_eth_checkpoint(state: &mut State, sync: keel_lc_eth::SyncCommitteeState) {
    state.vaults.eth_sync = Some(sync);
}

pub fn set_token_contract(state: &mut State, asset: Asset, contract: Vec<u8>) {
    state.vaults.token_contracts.insert(asset, contract);
}
