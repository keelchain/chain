//! Fresh-pool rotation for client-owned custody vaults (docs/models.md,
//! "Rotating to a fresh pool"). A custodial client that is labelled,
//! sanctioned or rebranding moves its whole platform to a new vault key of
//! its own: every account gets new deposit addresses on the bridged chain,
//! while its Keel accounts, balances and integration stay as they are.
//!
//! A pool is the client's vault on one chain; a *generation* is one key of
//! it. Exactly one generation is `Active` (the key `custody.vaults` points
//! at, so every address the RPC hands out and every new batch uses it);
//! the previous ones are `Draining`: still watched, still credited, still
//! paying withdrawals (one generation per transaction, never mixed) until
//! `grace_blocks` have passed and the observers report them empty, then
//! `Retired`. With `sweep` the observers also move the old coins to the
//! active generation's own address; without it (the taint case) nothing
//! ever links the two pools on the external chain.
//!
//! Generation 0 is the vault as registered; it is materialised here only
//! when the first rotation happens.

use crate::{
    context::BlockContext,
    modules::{clients, custody, staking, tokens, vaults},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, Chain, CustodyRotation, SweepObservation};
use keel_attest::Quorum;
use keel_types::{Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub enum GenStatus {
    Active,
    Draining,
    Retired,
}

impl GenStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GenStatus::Active => "Active",
            GenStatus::Draining => "Draining",
            GenStatus::Retired => "Retired",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Generation {
    pub gen: u64,
    pub public_key: Vec<u8>,
    pub chain_code: [u8; 32],
    pub signer_url: String,
    pub status: GenStatus,
    pub opened_height: u64,
    /// Height after which a draining generation may be retired.
    pub drain_until: u64,
    /// Whether the observers sweep its coins to the active generation.
    pub sweep: bool,
    /// Coins the observers swept out of it, per asset.
    pub swept: BTreeMap<Asset, Amount>,
    /// Observers that reported it empty.
    pub reported_empty: BTreeSet<Address>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Rotation {
    pub gen: u64,
    pub public_key: Vec<u8>,
    pub chain_code: [u8; 32],
    pub signer_url: String,
    pub activate_at: u64,
    pub sweep: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Pool {
    pub chain: Chain,
    pub custodian: Address,
    /// Every generation once a rotation happened (gen 0 included).
    pub gens: Vec<Generation>,
    pub next_gen: u64,
    pub rotation: Option<Rotation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct PoolsParams {
    /// Blocks a draining generation is kept at least.
    pub grace_blocks: u64,
    /// KEEL charged to the custodian per rotation.
    pub usage_rotation_keel: u128,
}

impl Default for PoolsParams {
    fn default() -> Self {
        Self {
            grace_blocks: 1_000,
            usage_rotation_keel: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct PoolsState {
    pub params: PoolsParams,
    pub pools: BTreeMap<(Chain, Address), Pool>,
    /// Batch id -> the generation the observers should pay it from.
    pub batch_gen: BTreeMap<u64, u64>,
    /// Sweep transactions already credited: (chain, tx hash).
    pub swept: BTreeSet<(Chain, [u8; 32])>,
    /// Sweep observations collecting votes, by observation digest (which
    /// leaves the fee out: observers derive it from the transaction, and
    /// the first vote's figure is the one charged). The observation is
    /// kept so the other observers can verify it on their own nodes and
    /// vote (`GET /v1/custody/{addr}` lists them).
    pub sweeps_pending: BTreeMap<[u8; 32], (Quorum, SweepObservation)>,
}

pub const PARAM_KEYS: &[&str] = &["pools.grace_blocks", "pools.usage_rotation_keel"];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    let p = &state.pools.params;
    Some(match key {
        "pools.grace_blocks" => p.grace_blocks as u128,
        "pools.usage_rotation_keel" => p.usage_rotation_keel,
        _ => return None,
    })
}

/// `SetParam` for the `pools.*` keys.
pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    let p = &mut state.pools.params;
    match key {
        "pools.grace_blocks" => {
            p.grace_blocks = u64::try_from(value)
                .ok()
                .filter(|b| *b >= 1)
                .ok_or_else(|| VmError::Invalid(format!("{key} must be >= 1")))?
        }
        "pools.usage_rotation_keel" => p.usage_rotation_keel = value,
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// A generation as the RPC and the observers see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenView {
    pub gen: u64,
    pub public_key: Vec<u8>,
    pub chain_code: [u8; 32],
    pub signer_url: String,
    pub status: GenStatus,
    pub opened_height: u64,
    pub drain_until: u64,
    pub sweep: bool,
    pub swept: BTreeMap<Asset, Amount>,
    pub reported_empty: usize,
}

fn view(g: &Generation) -> GenView {
    GenView {
        gen: g.gen,
        public_key: g.public_key.clone(),
        chain_code: g.chain_code,
        signer_url: g.signer_url.clone(),
        status: g.status,
        opened_height: g.opened_height,
        drain_until: g.drain_until,
        sweep: g.sweep,
        swept: g.swept.clone(),
        reported_empty: g.reported_empty.len(),
    }
}

/// Every generation of a client's vault on `chain`, gen 0 synthesized
/// from the registration while nothing was ever rotated.
pub fn generations(state: &State, chain: Chain, custodian: Address) -> Vec<GenView> {
    if let Some(p) = state.pools.pools.get(&(chain, custodian)) {
        if !p.gens.is_empty() {
            return p.gens.iter().map(view).collect();
        }
    }
    let Some(v) = state.custody.vault(chain, custodian) else {
        return Vec::new();
    };
    let Some(cc) = v.chain_code else {
        return Vec::new();
    };
    vec![GenView {
        gen: 0,
        public_key: v.public_key.clone(),
        chain_code: cc,
        signer_url: v.signer_url.clone(),
        status: GenStatus::Active,
        opened_height: v.registered_height,
        drain_until: 0,
        sweep: false,
        swept: BTreeMap::new(),
        reported_empty: 0,
    }]
}

/// Active and draining generations: what the observers watch and may pay from.
pub fn live(state: &State, chain: Chain, custodian: Address) -> Vec<GenView> {
    generations(state, chain, custodian)
        .into_iter()
        .filter(|g| g.status != GenStatus::Retired)
        .collect()
}

pub fn active(state: &State, chain: Chain, custodian: Address) -> Option<GenView> {
    generations(state, chain, custodian)
        .into_iter()
        .find(|g| g.status == GenStatus::Active)
}

/// Sweep observations of `custodian`'s vault on `chain` still collecting
/// votes, with the votes so far.
pub fn pending_sweeps(
    state: &State,
    chain: Chain,
    custodian: Address,
) -> Vec<(SweepObservation, u32)> {
    let (observers, _) = staking::observers(state);
    state
        .pools
        .sweeps_pending
        .values()
        .filter(|(_, o)| o.chain == chain && o.custodian == custodian)
        .map(|(q, o)| (o.clone(), q.count_within(&observers)))
        .collect()
}

pub fn rotation(state: &State, chain: Chain, custodian: Address) -> Option<Rotation> {
    state
        .pools
        .pools
        .get(&(chain, custodian))
        .and_then(|p| p.rotation.clone())
}

/// The generation a new batch of `custodian`'s withdrawals on `chain`
/// should be paid from: the oldest draining generation that is not being
/// swept (so it drains through ordinary payouts), else the active one.
/// A preference: an observer that finds it short pays from the active key.
pub fn batch_generation(state: &State, chain: Chain, custodian: Address) -> u64 {
    let gens = generations(state, chain, custodian);
    gens.iter()
        .find(|g| g.status == GenStatus::Draining && !g.sweep)
        .or_else(|| gens.iter().find(|g| g.status == GenStatus::Active))
        .map(|g| g.gen)
        .unwrap_or(0)
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::RotateCustodyVault(r) => rotate(state, ctx, signer, tx_id, r),
        Action::CancelCustodyRotation { chain } => cancel(state, signer, *chain),
        Action::ObserveCustodySweep(o) => observe_sweep(state, signer, o),
        Action::RetireCustodyGeneration {
            chain,
            custodian,
            gen,
        } => retire(state, ctx, signer, *chain, *custodian, *gen),
        _ => Err(VmError::Invalid("not a pools action".into())),
    }
}

fn rotate(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    r: &CustodyRotation,
) -> Result<Vec<Event>, VmError> {
    let vault = state
        .custody
        .vault(r.chain, signer)
        .cloned()
        .ok_or_else(|| VmError::NotFound("register a custody vault on this chain first".into()))?;
    if r.public_key.len() != r.chain.key_length() {
        return Err(VmError::Invalid("bad vault public key length".into()));
    }
    if r.public_key == vault.public_key {
        return Err(VmError::Invalid(
            "the fresh key must differ from the active one".into(),
        ));
    }
    if !custody::valid_signer_url(&r.signer_url) {
        return Err(VmError::Invalid("signer_url must be an http(s) URL".into()));
    }
    if r.signer_url == vault.signer_url {
        return Err(VmError::Invalid(
            "the fresh key needs its own signer URL (the old one keeps serving the draining generation)".into(),
        ));
    }
    if let Some(h) = r.activate_at {
        if h <= ctx.height {
            return Err(VmError::Invalid("activate_at must be in the future".into()));
        }
    }
    if rotation(state, r.chain, signer).is_some() {
        return Err(VmError::Invalid("a rotation is already pending".into()));
    }
    let mut events = clients::charge_usage_to(state, signer, "rotation", tx_id)?;
    let pool = state
        .pools
        .pools
        .entry((r.chain, signer))
        .or_insert_with(|| Pool {
            chain: r.chain,
            custodian: signer,
            gens: Vec::new(),
            next_gen: 1,
            rotation: None,
        });
    match r.activate_at {
        Some(h) => {
            let gen = pool.next_gen;
            pool.rotation = Some(Rotation {
                gen,
                public_key: r.public_key.clone(),
                chain_code: r.chain_code,
                signer_url: r.signer_url.clone(),
                activate_at: h,
                sweep: r.sweep,
            });
            events.push(Event::PoolRotationScheduled {
                chain: r.chain.as_str().into(),
                custodian: signer,
                gen,
                activate_at: h,
            });
        }
        None => events.extend(open_generation(
            state,
            ctx.height,
            r.chain,
            signer,
            r.public_key.clone(),
            r.chain_code,
            r.signer_url.clone(),
            r.sweep,
        )),
    }
    Ok(events)
}

#[allow(clippy::too_many_arguments)]
fn open_generation(
    state: &mut State,
    height: u64,
    chain: Chain,
    custodian: Address,
    public_key: Vec<u8>,
    chain_code: [u8; 32],
    signer_url: String,
    sweep: bool,
) -> Vec<Event> {
    let grace = state.pools.params.grace_blocks;
    let Some(old) = state.custody.vaults.get(&(chain, custodian)).cloned() else {
        return Vec::new();
    };
    let pool = state
        .pools
        .pools
        .entry((chain, custodian))
        .or_insert_with(|| Pool {
            chain,
            custodian,
            gens: Vec::new(),
            next_gen: 1,
            rotation: None,
        });
    let drain_until = height.saturating_add(grace);
    let previous = if pool.gens.is_empty() {
        pool.gens.push(Generation {
            gen: 0,
            public_key: old.public_key.clone(),
            chain_code: old.chain_code.unwrap_or([0u8; 32]),
            signer_url: old.signer_url.clone(),
            status: GenStatus::Draining,
            opened_height: old.registered_height,
            drain_until,
            sweep,
            swept: BTreeMap::new(),
            reported_empty: BTreeSet::new(),
        });
        0
    } else {
        let mut prev = 0;
        for g in pool.gens.iter_mut() {
            if g.status == GenStatus::Active {
                g.status = GenStatus::Draining;
                g.drain_until = drain_until;
                g.sweep = sweep;
                prev = g.gen;
            }
        }
        prev
    };
    let gen = pool.next_gen;
    pool.next_gen += 1;
    pool.gens.push(Generation {
        gen,
        public_key: public_key.clone(),
        chain_code,
        signer_url: signer_url.clone(),
        status: GenStatus::Active,
        opened_height: height,
        drain_until: 0,
        sweep: false,
        swept: BTreeMap::new(),
        reported_empty: BTreeSet::new(),
    });
    pool.rotation = None;
    // The registration record now carries the fresh key: everything that
    // derives addresses or signs new batches follows it.
    state.custody.vaults.insert(
        (chain, custodian),
        custody::CustodyVault {
            custodian,
            chain,
            epoch: old.epoch + 1,
            public_key,
            chain_code: Some(chain_code),
            signer_url,
            registered_height: height,
        },
    );
    vec![Event::PoolRotated {
        chain: chain.as_str().into(),
        custodian,
        gen,
        previous,
        drain_until,
    }]
}

fn cancel(state: &mut State, signer: Address, chain: Chain) -> Result<Vec<Event>, VmError> {
    let pool = state
        .pools
        .pools
        .get_mut(&(chain, signer))
        .ok_or_else(|| VmError::NotFound("no pool".into()))?;
    let r = pool
        .rotation
        .take()
        .ok_or_else(|| VmError::Invalid("no pending rotation".into()))?;
    Ok(vec![Event::PoolRotationCancelled {
        chain: chain.as_str().into(),
        custodian: signer,
        gen: r.gen,
    }])
}

fn observe_sweep(
    state: &mut State,
    signer: Address,
    o: &SweepObservation,
) -> Result<Vec<Event>, VmError> {
    let (observers, threshold) = vaults::require_observer(state, &signer)?;
    if state.pools.swept.contains(&(o.chain, o.tx_hash)) {
        return Err(VmError::Invalid("sweep already recorded".into()));
    }
    if vaults::chain_of(state, &o.asset)? != o.chain {
        return Err(VmError::Invalid("asset is not on that chain".into()));
    }
    let from = generations(state, o.chain, o.custodian)
        .into_iter()
        .find(|g| g.gen == o.from_gen)
        .ok_or_else(|| VmError::NotFound(format!("generation {}", o.from_gen)))?;
    if from.status != GenStatus::Draining {
        return Err(VmError::Invalid(
            "only a draining generation is swept".into(),
        ));
    }
    let digest = keel_crypto::sha256(&[
        b"custody-sweep-v1",
        &borsh::to_vec(&(
            o.chain,
            o.custodian,
            o.from_gen,
            o.tx_hash,
            o.asset.clone(),
            o.amount,
        ))
        .map_err(|e| VmError::Invalid(e.to_string()))?,
    ]);
    let (q, first) = state
        .pools
        .sweeps_pending
        .entry(digest)
        .or_insert_with(|| (Quorum::default(), o.clone()));
    if !q.vote(signer) {
        return Err(VmError::Invalid("observer already voted".into()));
    }
    let fee = first.fee;
    if !q.reached(&observers, threshold) {
        return Ok(Vec::new());
    }
    state.pools.sweeps_pending.remove(&digest);
    state.pools.swept.insert((o.chain, o.tx_hash));
    if fee > 0 {
        let native = vaults::native_asset(o.chain);
        custody::charge_network_fee(
            state,
            o.custodian,
            &native,
            fee,
            &format!("sweep:{}", tokens::hex(&o.tx_hash)),
        )?;
    }
    if let Some(p) = state.pools.pools.get_mut(&(o.chain, o.custodian)) {
        if let Some(g) = p.gens.iter_mut().find(|g| g.gen == o.from_gen) {
            let s = g.swept.entry(o.asset.clone()).or_insert(0);
            *s = s.saturating_add(o.amount);
        }
    }
    Ok(vec![Event::PoolSwept {
        chain: o.chain.as_str().into(),
        custodian: o.custodian,
        from_gen: o.from_gen,
        asset: o.asset.clone(),
        amount: o.amount,
        fee,
        tx_hash: tokens::hex(&o.tx_hash),
    }])
}

fn retire(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    chain: Chain,
    custodian: Address,
    gen: u64,
) -> Result<Vec<Event>, VmError> {
    let is_observer = staking::is_observer(state, &signer);
    if signer != custodian && !is_observer {
        return Err(VmError::Unauthorized);
    }
    let (observers, threshold) = staking::observers(state);
    let pool = state
        .pools
        .pools
        .get_mut(&(chain, custodian))
        .ok_or_else(|| VmError::NotFound("no pool".into()))?;
    let g = pool
        .gens
        .iter_mut()
        .find(|g| g.gen == gen)
        .ok_or_else(|| VmError::NotFound(format!("generation {gen}")))?;
    if g.status != GenStatus::Draining {
        return Err(VmError::Invalid(
            "only a draining generation retires".into(),
        ));
    }
    if ctx.height < g.drain_until {
        return Err(VmError::Invalid(format!(
            "generation drains until height {}",
            g.drain_until
        )));
    }
    if signer != custodian {
        g.reported_empty.insert(signer);
        let reports = g
            .reported_empty
            .iter()
            .filter(|a| observers.contains(a))
            .count() as u32;
        if reports < threshold {
            return Ok(Vec::new());
        }
    }
    g.status = GenStatus::Retired;
    Ok(vec![Event::PoolGenerationRetired {
        chain: chain.as_str().into(),
        custodian,
        gen,
    }])
}

/// Open the rotations whose activation height came, and retire draining
/// generations whose grace is over and that the observer quorum reported
/// empty.
pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = Vec::new();
    let due: Vec<((Chain, Address), Rotation)> = state
        .pools
        .pools
        .iter()
        .filter_map(|(k, p)| {
            p.rotation
                .clone()
                .filter(|r| r.activate_at <= ctx.height)
                .map(|r| (*k, r))
        })
        .collect();
    for ((chain, custodian), r) in due {
        events.extend(open_generation(
            state,
            ctx.height,
            chain,
            custodian,
            r.public_key,
            r.chain_code,
            r.signer_url,
            r.sweep,
        ));
    }
    let (observers, threshold) = staking::observers(state);
    let mut retired = Vec::new();
    for (k, p) in state.pools.pools.iter_mut() {
        for g in p.gens.iter_mut() {
            if g.status != GenStatus::Draining || ctx.height < g.drain_until {
                continue;
            }
            let reports = g
                .reported_empty
                .iter()
                .filter(|a| observers.contains(a))
                .count() as u32;
            if reports >= threshold {
                g.status = GenStatus::Retired;
                retired.push((*k, g.gen));
            }
        }
    }
    for ((chain, custodian), gen) in retired {
        events.push(Event::PoolGenerationRetired {
            chain: chain.as_str().into(),
            custodian,
            gen,
        });
    }
    events
}
