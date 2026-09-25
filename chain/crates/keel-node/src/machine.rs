//! `VmMachine`: the VM behind consensus, plus the mempool, receipts and
//! durable storage (block journal + snapshots) that let a node restart
//! from its own records.
//!
//! Durability rule: a finalized block is appended to the journal BEFORE
//! `apply` returns (and therefore before marshal receives the ack). On
//! restart the newest snapshot is loaded and the journal replayed above
//! it, so `tip()` is always the last durably applied height.
//!
//! Above the VM: tokio, HashMap and the wall clock are allowed here.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use anyhow::Context as _;
use borsh::{BorshDeserialize, BorshSerialize};
use bytes::Bytes;
use commonware_consensus::types::Height;
use commonware_cryptography::sha256::Digest;
use keel_actions::{decode_payload, encode_payload, SignedAction};
use keel_consensus::StateMachine;
use keel_rpc::BlockUpdate;
use keel_types::Address;
use keel_vm::{apply_block, check_admission, receipt::Receipt, BlockContext, State, VmError};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Actions per block the proposer will include at most.
pub const MAX_ACTIONS_PER_PROPOSAL: usize = 5_000;
/// How far ahead of the on-chain nonce a signer may queue actions.
pub const NONCE_WINDOW: u64 = 64;
/// Mempool entries older than this are dropped.
pub const MEMPOOL_TTL_SECS: u64 = 600;
/// Blocks of receipts kept in memory.
pub const RECEIPT_WINDOW: u64 = 10_000;
/// An action selected into a proposal is not re-selected for this long,
/// so one node does not include it twice while its block is in flight.
pub const INFLIGHT_SECS: u64 = 3;

// ---------------- mempool ----------------

struct Entry {
    action: SignedAction,
    inserted: Instant,
}

/// Per-signer, nonce-ordered pending actions.
#[derive(Default)]
pub struct Mempool {
    by_signer: BTreeMap<Address, BTreeMap<u64, Entry>>,
    ids: HashSet<[u8; 32]>,
    inflight: HashMap<[u8; 32], Instant>,
    len: usize,
}

impl Mempool {
    pub fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Admitted actions not yet in a proposal in flight: what a block built
    /// now would carry.
    pub fn has_selectable(&self) -> bool {
        self.len > self.inflight.len()
    }

    /// Admit an action. Signature, chain and budget are checked against
    /// `state`; the nonce may run ahead of the chain by `NONCE_WINDOW` so a
    /// client can pipeline. Returns `Ok(true)` when newly inserted.
    pub fn insert(&mut self, state: &State, action: SignedAction) -> Result<bool, VmError> {
        let id = action.id();
        if self.ids.contains(&id) {
            return Ok(false);
        }
        let signer = action.signer();
        let chain_nonce = state.account_ref(&signer).map(|m| m.nonce).unwrap_or(0);
        let nonce = action.envelope.nonce;
        if nonce == chain_nonce {
            check_admission(state, &action)?;
        } else {
            if nonce < chain_nonce || nonce >= chain_nonce + NONCE_WINDOW {
                return Err(VmError::BadNonce {
                    expected: chain_nonce,
                    got: nonce,
                });
            }
            if action.envelope.chain_id != state.chain_id {
                return Err(VmError::WrongChain);
            }
            if !action.verify() {
                return Err(VmError::BadSignature);
            }
        }
        let queue = self.by_signer.entry(signer).or_default();
        if queue.contains_key(&nonce) {
            return Err(VmError::Invalid("nonce already queued".into()));
        }
        queue.insert(
            nonce,
            Entry {
                action,
                inserted: Instant::now(),
            },
        );
        self.ids.insert(id);
        self.len += 1;
        Ok(true)
    }

    /// Consecutive runs of nonces per signer starting at the chain nonce,
    /// bounded per signer by the per-block cap and in total by `max`.
    pub fn select(&self, state: &State, max: usize) -> Vec<SignedAction> {
        let per_signer = state.params.budget.max_per_block as usize;
        let now = Instant::now();
        let mut out = Vec::new();
        for (signer, queue) in &self.by_signer {
            let mut expected = state.account_ref(signer).map(|m| m.nonce).unwrap_or(0);
            let mut taken = 0usize;
            while let Some(e) = queue.get(&expected) {
                if taken >= per_signer || out.len() >= max {
                    break;
                }
                // A nonce run stops at an in-flight action: later nonces
                // would fail without it.
                if self
                    .inflight
                    .get(&e.action.id())
                    .is_some_and(|t| now.duration_since(*t).as_secs() < INFLIGHT_SECS)
                {
                    break;
                }
                out.push(e.action.clone());
                expected += 1;
                taken += 1;
            }
            if out.len() >= max {
                break;
            }
        }
        out
    }

    /// Remember that `actions` went into a proposal.
    pub fn mark_inflight(&mut self, actions: &[SignedAction]) {
        let now = Instant::now();
        for a in actions {
            self.inflight.insert(a.id(), now);
        }
    }

    /// Drop included ids, stale nonces and expired entries.
    pub fn prune(&mut self, state: &State, included: &[[u8; 32]]) {
        let included: HashSet<&[u8; 32]> = included.iter().collect();
        let now = Instant::now();
        self.inflight.retain(|id, t| {
            !included.contains(id) && now.duration_since(*t).as_secs() < INFLIGHT_SECS * 4
        });
        let mut empty = Vec::new();
        for (signer, queue) in self.by_signer.iter_mut() {
            let chain_nonce = state.account_ref(signer).map(|m| m.nonce).unwrap_or(0);
            queue.retain(|nonce, e| {
                let keep = *nonce >= chain_nonce
                    && !included.contains(&e.action.id())
                    && now.duration_since(e.inserted).as_secs() < MEMPOOL_TTL_SECS;
                if !keep {
                    self.ids.remove(&e.action.id());
                    self.len -= 1;
                }
                keep
            });
            if queue.is_empty() {
                empty.push(*signer);
            }
        }
        for s in empty {
            self.by_signer.remove(&s);
        }
    }
}

// ---------------- receipts ----------------

#[derive(Default)]
pub struct ReceiptIndex {
    by_tx: HashMap<[u8; 32], Receipt>,
    by_height: BTreeMap<u64, Vec<[u8; 32]>>,
    /// End-of-block events per height (same window as receipts).
    events_by_height: BTreeMap<u64, Vec<keel_vm::Event>>,
}

impl ReceiptIndex {
    pub fn events_at(&self, height: u64) -> Option<Vec<keel_vm::Event>> {
        self.events_by_height.get(&height).cloned()
    }

    fn insert_events(&mut self, height: u64, events: &[keel_vm::Event]) {
        if !events.is_empty() {
            self.events_by_height.insert(height, events.to_vec());
        }
        while self.events_by_height.len() as u64 > RECEIPT_WINDOW {
            self.events_by_height.pop_first();
        }
    }

    fn insert(&mut self, height: u64, receipts: &[Receipt]) {
        let ids: Vec<[u8; 32]> = receipts.iter().map(|r| r.tx_id).collect();
        for r in receipts {
            // The same action can land in two blocks (two proposers built
            // from the same mempool). Only the first inclusion consumed the
            // nonce, so it is the receipt that mattered: a later BAD_NONCE
            // is duplicate-inclusion noise and never replaces it. The one
            // exception is an early inclusion that itself failed BAD_NONCE
            // (arrived before its predecessor) and was re-included later.
            let dup_noise = matches!(r.error, Some(VmError::BadNonce { .. }));
            match self.by_tx.get(&r.tx_id) {
                Some(existing) if !matches!(existing.error, Some(VmError::BadNonce { .. })) => {
                    let _ = existing;
                }
                Some(_) if dup_noise => {}
                _ => {
                    self.by_tx.insert(r.tx_id, r.clone());
                }
            }
        }
        self.by_height.insert(height, ids);
        while self.by_height.len() as u64 > RECEIPT_WINDOW {
            if let Some((_, old)) = self.by_height.pop_first() {
                for id in old {
                    self.by_tx.remove(&id);
                }
            }
        }
    }

    pub fn get(&self, tx_id: &[u8; 32]) -> Option<Receipt> {
        self.by_tx.get(tx_id).cloned()
    }

    pub fn at(&self, height: u64) -> Option<Vec<Receipt>> {
        self.by_height.get(&height).map(|ids| {
            ids.iter()
                .filter_map(|id| self.by_tx.get(id).cloned())
                .collect()
        })
    }
}

// ---------------- storage ----------------

#[derive(BorshSerialize, BorshDeserialize)]
struct JournalEntry {
    height: u64,
    timestamp: u64,
    payload: Vec<u8>,
}

pub struct Storage {
    dir: PathBuf,
    journal: File,
    receipts: File,
}

impl Storage {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let journal = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(dir.join("journal.bin"))?;
        let receipts = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("receipts.jsonl"))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            journal,
            receipts,
        })
    }

    /// Appends and returns (offset of the entry body, body length) for the
    /// archive index.
    fn append(&mut self, entry: &JournalEntry) -> anyhow::Result<(u64, u32)> {
        let bytes = borsh::to_vec(entry)?;
        let start = self.journal.seek(SeekFrom::End(0))?;
        self.journal
            .write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.journal.write_all(&bytes)?;
        self.journal.flush()?;
        let body = (start + 4, bytes.len() as u32);
        self.journal.sync_data()?;
        Ok(body)
    }

    /// Appends one JSON line per receipt; returns the bytes written.
    fn append_receipts(&mut self, height: u64, receipts: &[Receipt]) -> anyhow::Result<u64> {
        let mut written = 0u64;
        for r in receipts {
            let line = serde_json::json!({ "height": height, "receipt": r }).to_string();
            self.receipts.write_all(line.as_bytes())?;
            self.receipts.write_all(b"\n")?;
            written += line.len() as u64 + 1;
        }
        self.receipts.flush()?;
        Ok(written)
    }

    /// Every journal entry with height > `above`, in order. Truncated or
    /// corrupt tails are ignored (a crash mid-write).
    fn replay_above(&mut self, above: u64) -> anyhow::Result<Vec<JournalEntry>> {
        let mut out = Vec::new();
        let mut buf = Vec::new();
        self.journal.seek(SeekFrom::Start(0))?;
        self.journal.read_to_end(&mut buf)?;
        let mut pos = 0usize;
        while pos + 4 <= buf.len() {
            let len =
                u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
            pos += 4;
            if pos + len > buf.len() {
                warn!("journal tail truncated; ignoring");
                break;
            }
            match JournalEntry::try_from_slice(&buf[pos..pos + len]) {
                Ok(e) => {
                    if e.height > above {
                        out.push(e);
                    }
                }
                Err(_) => {
                    warn!("journal entry corrupt; stopping replay");
                    break;
                }
            }
            pos += len;
        }
        Ok(out)
    }

    fn write_snapshot(&self, height: u64, state: &State) -> anyhow::Result<()> {
        let tmp = self.dir.join(format!("snapshot-{height}.tmp"));
        let path = self.dir.join(format!("snapshot-{height}.bin"));
        fs::write(&tmp, state.snapshot())?;
        fs::rename(&tmp, &path)?;
        // Keep the two newest snapshots.
        let mut olds: Vec<(u64, PathBuf)> = self
            .snapshots()?
            .into_iter()
            .filter(|(h, _)| *h < height)
            .collect();
        olds.sort();
        while olds.len() > 1 {
            let (_, p) = olds.remove(0);
            let _ = fs::remove_file(p);
        }
        Ok(())
    }

    fn snapshots(&self) -> anyhow::Result<Vec<(u64, PathBuf)>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(h) = name
                .strip_prefix("snapshot-")
                .and_then(|s| s.strip_suffix(".bin"))
            {
                if let Ok(h) = h.parse::<u64>() {
                    out.push((h, entry.path()));
                }
            }
        }
        Ok(out)
    }

    fn newest_snapshot(&self) -> anyhow::Result<Option<(u64, State)>> {
        let mut snaps = self.snapshots()?;
        snaps.sort();
        while let Some((h, p)) = snaps.pop() {
            match fs::read(&p).ok().and_then(|b| State::restore(&b)) {
                Some(s) => return Ok(Some((h, s))),
                None => warn!(path = %p.display(), "snapshot unreadable; trying older"),
            }
        }
        Ok(None)
    }
}

// ---------------- machine ----------------

pub struct VmMachine {
    state: Arc<Mutex<State>>,
    mempool: Arc<Mutex<Mempool>>,
    receipts: Arc<Mutex<ReceiptIndex>>,
    storage: Storage,
    archive: Arc<Mutex<crate::archive::Archive>>,
    tip: (Height, Digest),
    snapshot_interval: u64,
    updates: broadcast::Sender<BlockUpdate>,
}

/// Handles the RPC and gossip share with the machine.
#[derive(Clone)]
pub struct Shared {
    pub state: Arc<Mutex<State>>,
    pub mempool: Arc<Mutex<Mempool>>,
    pub receipts: Arc<Mutex<ReceiptIndex>>,
    pub updates: broadcast::Sender<BlockUpdate>,
    /// Full history from the journals (any height).
    pub archive: Arc<Mutex<crate::archive::Archive>>,
}

fn digest_of(hash: [u8; 32]) -> Digest {
    Digest(hash)
}

impl VmMachine {
    /// Open storage under `dir`; start from `genesis` unless a snapshot or
    /// journal exists, in which case the durable state wins.
    pub fn open(
        dir: &Path,
        genesis: State,
        snapshot_interval: u64,
    ) -> anyhow::Result<(Self, Shared)> {
        let mut storage = Storage::open(dir)?;
        let (mut state, mut height) = match storage.newest_snapshot()? {
            Some((h, s)) => {
                info!(height = h, "loaded snapshot");
                (s, h)
            }
            None => (genesis, 0),
        };
        let entries = storage.replay_above(height)?;
        let replayed = entries.len();
        for e in entries {
            if e.height != height + 1 {
                warn!(
                    expected = height + 1,
                    got = e.height,
                    "journal gap; stopping replay"
                );
                break;
            }
            let actions = decode_payload(&e.payload).unwrap_or_default();
            let ctx = BlockContext {
                height: e.height,
                timestamp: e.timestamp,
                proposer: None,
            };
            apply_block(&mut state, &ctx, &actions);
            height = e.height;
        }
        if replayed > 0 {
            info!(replayed, height, "replayed journal");
        }
        let tip = (Height::new(height), digest_of(state.last_hash));
        let (updates, _) = broadcast::channel(256);
        let archive = crate::archive::Archive::open(dir)?;
        info!(archived = archive.tip(), "opened block archive");
        let shared = Shared {
            state: Arc::new(Mutex::new(state)),
            mempool: Arc::new(Mutex::new(Mempool::default())),
            receipts: Arc::new(Mutex::new(ReceiptIndex::default())),
            updates: updates.clone(),
            archive: Arc::new(Mutex::new(archive)),
        };
        let machine = Self {
            state: shared.state.clone(),
            mempool: shared.mempool.clone(),
            receipts: shared.receipts.clone(),
            archive: shared.archive.clone(),
            storage,
            tip,
            snapshot_interval: snapshot_interval.max(1),
            updates,
        };
        Ok((machine, shared))
    }
}

impl StateMachine for VmMachine {
    fn build(&mut self, _parent_height: Height, _timestamp: u64) -> Bytes {
        let state = self.state.lock().expect("state lock");
        let mut mempool = self.mempool.lock().expect("mempool lock");
        let actions = mempool.select(&state, MAX_ACTIONS_PER_PROPOSAL);
        if actions.is_empty() {
            return Bytes::new();
        }
        mempool.mark_inflight(&actions);
        Bytes::from(encode_payload(&actions))
    }

    fn check(&self, payload: &[u8]) -> bool {
        decode_payload(payload).is_some()
    }

    fn block_intervals_ms(&self) -> (u64, u64) {
        let p = &self.state.lock().expect("state lock").params;
        (
            u64::from(p.min_block_interval_ms),
            u64::from(p.idle_block_interval_ms),
        )
    }

    fn has_pending(&self) -> bool {
        self.mempool.lock().expect("mempool lock").has_selectable()
    }

    fn apply(&mut self, height: Height, timestamp: u64, payload: &[u8]) -> Digest {
        if height <= self.tip.0 {
            return self.tip.1;
        }
        let actions = decode_payload(payload).unwrap_or_default();
        let ctx = BlockContext {
            height: height.get(),
            timestamp,
            proposer: None,
        };
        let (receipts, events, hash, journal_pos) = {
            let mut state = self.state.lock().expect("state lock");
            let (receipts, events) = apply_block(&mut state, &ctx, &actions);
            let hash = state.last_hash;
            // Durable before ack.
            let journal_pos = match self.storage.append(&JournalEntry {
                height: height.get(),
                timestamp,
                payload: payload.to_vec(),
            }) {
                Ok(pos) => pos,
                // Without a durable record a restart would fork this node
                // from its own history; halting is the safe choice.
                Err(e) => panic!("journal append failed: {e}"),
            };
            if height.get().is_multiple_of(self.snapshot_interval) {
                if let Err(e) = self.storage.write_snapshot(height.get(), &state) {
                    warn!(?e, "snapshot failed");
                }
            }
            let included: Vec<[u8; 32]> = receipts.iter().map(|r| r.tx_id).collect();
            self.mempool
                .lock()
                .expect("mempool lock")
                .prune(&state, &included);
            (receipts, events, hash, journal_pos)
        };
        let receipt_bytes = match self.storage.append_receipts(height.get(), &receipts) {
            Ok(n) => n,
            Err(e) => {
                warn!(?e, "receipt journal append failed");
                0
            }
        };
        if let Err(e) = self.archive.lock().expect("archive lock").record(
            height.get(),
            actions.len() as u32,
            hash,
            journal_pos.0,
            journal_pos.1,
            receipt_bytes,
        ) {
            warn!(?e, "archive record failed");
        }
        {
            let mut idx = self.receipts.lock().expect("receipts lock");
            idx.insert(height.get(), &receipts);
            idx.insert_events(height.get(), &events);
        }
        let _ = self.updates.send(BlockUpdate {
            height: height.get(),
            timestamp,
            state_hash: hex::encode(hash),
            receipts,
            events,
        });
        self.tip = (height, digest_of(hash));
        self.tip.1
    }

    fn tip(&self) -> (Height, Digest) {
        self.tip
    }

    /// The staking module's active set, by consensus key, in power order.
    /// Called right after an epoch's boundary block is applied, so the
    /// current state decides the next epoch.
    fn validators(&self, _epoch: u64) -> Option<Vec<[u8; 32]>> {
        let state = self.state.lock().expect("state lock");
        let set: Vec<[u8; 32]> = keel_vm::modules::staking::validator_set(&state)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        if set.is_empty() {
            None
        } else {
            Some(set)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_actions::{Action, Transfer, CHAIN_ID_DEVNET};
    use keel_crypto::Keypair;
    use keel_types::Asset;
    use keel_vm::Genesis;

    fn transfer(k: &Keypair, nonce: u64) -> SignedAction {
        SignedAction::sign(
            k,
            nonce,
            CHAIN_ID_DEVNET,
            Action::Transfer(Transfer {
                to: Address::tagged(9),
                asset: Asset::new("KEEL"),
                amount: 1,
                memo: None,
            }),
        )
    }

    #[test]
    fn mempool_pipelines_nonces_and_prunes() {
        let alice = Keypair::from_seed(1);
        let state = Genesis::devnet(CHAIN_ID_DEVNET, &[alice.address()], vec![]).build();
        let mut mp = Mempool::default();
        // Out of order arrival still selects in nonce order.
        assert!(mp.insert(&state, transfer(&alice, 2)).unwrap());
        assert!(mp.insert(&state, transfer(&alice, 0)).unwrap());
        assert!(
            !mp.insert(&state, transfer(&alice, 0)).unwrap(),
            "duplicate"
        );
        assert_eq!(
            mp.select(&state, 10).len(),
            1,
            "nonce 1 missing: only 0 selectable"
        );
        assert!(mp.insert(&state, transfer(&alice, 1)).unwrap());
        let sel = mp.select(&state, 10);
        assert_eq!(
            sel.iter().map(|a| a.envelope.nonce).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(matches!(
            mp.insert(&state, transfer(&alice, 500)),
            Err(VmError::BadNonce { .. })
        ));
        // After a block applied nonces 0..2, everything is pruned.
        let mut s2 = state.clone();
        apply_block(
            &mut s2,
            &BlockContext {
                height: 1,
                timestamp: 1,
                proposer: None,
            },
            &sel,
        );
        mp.prune(&s2, &[]);
        assert!(mp.is_empty());
    }

    #[test]
    fn journal_replay_restores_state_after_restart() {
        let alice = Keypair::from_seed(1);
        let dir = std::env::temp_dir().join(format!("keel-machine-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let genesis = Genesis::devnet(CHAIN_ID_DEVNET, &[alice.address()], vec![]);
        let (mut m, shared) = VmMachine::open(&dir, genesis.build(), 2).unwrap();
        for h in 1..=5u64 {
            shared
                .mempool
                .lock()
                .unwrap()
                .insert(&shared.state.lock().unwrap(), transfer(&alice, h - 1))
                .unwrap();
            let payload = m.build(Height::new(h - 1), h);
            m.apply(Height::new(h), 1_000 + h, &payload);
        }
        let (tip, hash) = m.tip();
        assert_eq!(tip.get(), 5);
        assert!(shared.receipts.lock().unwrap().at(5).is_some());
        drop(m);
        // Reopen: snapshot at 4 + journal 5.
        let (m2, shared2) = VmMachine::open(&dir, genesis.build(), 2).unwrap();
        assert_eq!(m2.tip(), (tip, hash));
        assert_eq!(
            shared2
                .state
                .lock()
                .unwrap()
                .account_ref(&alice.address())
                .unwrap()
                .nonce,
            5
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
