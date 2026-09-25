//! Ethereum sync-committee light-client verification for ERC-20 deposits.
//!
//! Proof format ([`EthDepositProof`]) and exactly what is verified:
//! 1. `attested` beacon header is signed by the sync committee: the
//!    participation bitfield names ≥ 2/3 of `committee_size` members, the
//!    supplied committee pubkeys + aggregate pubkey hash to the committee
//!    root stored for the current period, and the BLS12-381 aggregate
//!    signature over `compute_signing_root(header_root, DOMAIN_SYNC_COMMITTEE)`
//!    verifies (blst, min-pk, POP ciphersuite) — [`verify_sync_aggregate`].
//! 2. `finalized` header is proven under `attested.state_root` through the
//!    finality branch at the configured generalized index (105 for Altair
//!    through Deneb states).
//! 3. The execution payload header (its 17 field roots) is proven under
//!    `finalized.body_root` at generalized index 25; `receipts_root` and
//!    `block_number` are read from those field roots.
//! 4. The receipt at `tx_index` is proven under `receipts_root` with a
//!    Merkle-Patricia proof (alloy-trie), decoded (typed receipts
//!    supported), and the log at `log_index` must be
//!    `Transfer(address,address,uint256)` from `token` to `to` for `amount`.
//! 5. Sync-committee rotation: [`SyncCommitteeState::apply_committee_update`]
//!    accepts the next committee proven under a finalized state root at
//!    generalized index 55 and advances the period.
//!
//! NOT verified: that `attested` itself is in the canonical chain beyond
//! the committee's supermajority (sync-committee security, weaker than
//! full consensus, see docs/plan.md §4); the VM still requires the
//! observer quorum on top of this proof.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use alloy_primitives::{keccak256, Bytes, B256};
use alloy_rlp::Header as RlpHeader;
use alloy_trie::{proof::verify_proof, Nibbles};
use blst::{
    min_pk::{PublicKey, Signature},
    BLST_ERROR,
};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";
pub const DOMAIN_SYNC_COMMITTEE: [u8; 4] = [7, 0, 0, 0];
pub const SYNC_COMMITTEE_SIZE: u32 = 512;
/// finalized_checkpoint.root in BeaconState (Altair..Deneb).
pub const FINALIZED_ROOT_GINDEX: u64 = 105;
/// next_sync_committee in BeaconState (Altair..Deneb).
pub const NEXT_SYNC_COMMITTEE_GINDEX: u64 = 55;
/// execution_payload in BeaconBlockBody (Capella..Electra: 12-13 fields, depth 4).
pub const EXECUTION_PAYLOAD_GINDEX: u64 = 25;
pub const EXECUTION_HEADER_FIELDS: usize = 17;
pub const RECEIPTS_ROOT_FIELD: usize = 3;
pub const BLOCK_NUMBER_FIELD: usize = 6;
pub const EPOCHS_PER_SYNC_COMMITTEE_PERIOD: u64 = 256;
pub const SLOTS_PER_EPOCH: u64 = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("malformed proof: {0}")]
    Malformed(&'static str),
    #[error("committee pubkeys do not match the stored committee root")]
    CommitteeMismatch,
    #[error("insufficient participation: {0} of {1}")]
    Participation(u32, u32),
    #[error("bad BLS signature")]
    BadSignature,
    #[error("finality branch invalid")]
    FinalityBranch,
    #[error("execution payload branch invalid")]
    ExecutionBranch,
    #[error("receipt proof invalid")]
    ReceiptProof,
    #[error("receipt decode failed")]
    ReceiptDecode,
    #[error("log {0} is not the expected Transfer")]
    NotTransfer(u32),
    #[error("proof is for period {0}, state is at {1}")]
    WrongPeriod(u64, u64),
    #[error("next committee branch invalid")]
    CommitteeBranch,
    #[error("finalized slot not newer than the stored one")]
    NotNewer,
}

#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct BeaconHeader {
    pub slot: u64,
    pub proposer_index: u64,
    pub parent_root: [u8; 32],
    pub state_root: [u8; 32],
    pub body_root: [u8; 32],
}

impl BeaconHeader {
    pub fn hash_tree_root(&self) -> [u8; 32] {
        merkleize(
            &[
                u64_root(self.slot),
                u64_root(self.proposer_index),
                self.parent_root,
                self.state_root,
                self.body_root,
            ],
            8,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct EthDepositProof {
    pub attested: BeaconHeader,
    pub finalized: BeaconHeader,
    pub finality_branch: Vec<[u8; 32]>,
    /// Little-endian bitfield, one bit per committee member.
    pub sync_committee_bits: Vec<u8>,
    pub sync_committee_signature: Vec<u8>,
    /// Compressed G1 pubkeys (48 bytes each), committee order.
    pub committee_pubkeys: Vec<Vec<u8>>,
    pub committee_aggregate_pubkey: Vec<u8>,
    /// Field roots of the ExecutionPayloadHeader (17 for Deneb).
    pub execution_fields: Vec<[u8; 32]>,
    pub execution_branch: Vec<[u8; 32]>,
    pub tx_index: u64,
    pub log_index: u32,
    /// Raw receipt bytes (typed receipts keep their type prefix).
    pub receipt: Vec<u8>,
    pub receipt_proof: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedEth {
    pub finalized_slot: u64,
    pub block_number: u64,
    pub receipts_root: [u8; 32],
    pub token: [u8; 20],
    pub from: [u8; 20],
    pub to: [u8; 20],
    pub amount: u128,
    pub participation: u32,
}

/// What the VM stores per Ethereum vault (borsh). Trusted values come from
/// a governance checkpoint; `apply_committee_update` moves it forward.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SyncCommitteeState {
    pub period: u64,
    /// hash_tree_root(SyncCommittee{pubkeys, aggregate_pubkey}).
    pub committee_root: [u8; 32],
    pub next_committee_root: Option<[u8; 32]>,
    pub genesis_validators_root: [u8; 32],
    pub fork_version: [u8; 4],
    pub committee_size: u32,
    pub finalized_root_gindex: u64,
    pub next_sync_committee_gindex: u64,
    pub execution_payload_gindex: u64,
    pub last_finalized_slot: u64,
}

impl SyncCommitteeState {
    /// A bootstrap with an explicit committee size and optional next
    /// committee (genesis / governance checkpoints).
    pub fn bootstrap(
        period: u64,
        committee_root: [u8; 32],
        next_committee_root: Option<[u8; 32]>,
        genesis_validators_root: [u8; 32],
        fork_version: [u8; 4],
        committee_size: u32,
    ) -> Self {
        let mut s = Self::checkpoint(
            period,
            committee_root,
            genesis_validators_root,
            fork_version,
        );
        s.next_committee_root = next_committee_root;
        if committee_size > 0 {
            s.committee_size = committee_size;
        }
        s
    }

    pub fn checkpoint(
        period: u64,
        committee_root: [u8; 32],
        genesis_validators_root: [u8; 32],
        fork_version: [u8; 4],
    ) -> Self {
        Self {
            period,
            committee_root,
            next_committee_root: None,
            genesis_validators_root,
            fork_version,
            committee_size: SYNC_COMMITTEE_SIZE,
            finalized_root_gindex: FINALIZED_ROOT_GINDEX,
            next_sync_committee_gindex: NEXT_SYNC_COMMITTEE_GINDEX,
            execution_payload_gindex: EXECUTION_PAYLOAD_GINDEX,
            last_finalized_slot: 0,
        }
    }

    pub fn period_of_slot(slot: u64) -> u64 {
        slot / (SLOTS_PER_EPOCH * EPOCHS_PER_SYNC_COMMITTEE_PERIOD)
    }

    fn domain(&self) -> [u8; 32] {
        let mut fv = [0u8; 32];
        fv[..4].copy_from_slice(&self.fork_version);
        let fork_data_root = sha256_pair(&fv, &self.genesis_validators_root);
        let mut d = [0u8; 32];
        d[..4].copy_from_slice(&DOMAIN_SYNC_COMMITTEE);
        d[4..].copy_from_slice(&fork_data_root[..28]);
        d
    }

    /// Full verification of a deposit proof (steps 1-4 in the crate docs).
    pub fn verify_deposit(
        &mut self,
        proof: &EthDepositProof,
        token: [u8; 20],
    ) -> Result<VerifiedEth, Error> {
        let participation = self.verify_sync_aggregate(proof)?;
        // 2. finality
        let finalized_root = proof.finalized.hash_tree_root();
        if !verify_branch(
            &finalized_root,
            &proof.finality_branch,
            self.finalized_root_gindex,
            &proof.attested.state_root,
        ) {
            return Err(Error::FinalityBranch);
        }
        // 3. execution payload header under the finalized body root
        if proof.execution_fields.len() < BLOCK_NUMBER_FIELD + 1
            || proof.execution_fields.len() > 32
        {
            return Err(Error::Malformed("execution_fields"));
        }
        let payload_root = merkleize(&proof.execution_fields, 32);
        if !verify_branch(
            &payload_root,
            &proof.execution_branch,
            self.execution_payload_gindex,
            &proof.finalized.body_root,
        ) {
            return Err(Error::ExecutionBranch);
        }
        let receipts_root = proof.execution_fields[RECEIPTS_ROOT_FIELD];
        let block_number = u64_from_root(&proof.execution_fields[BLOCK_NUMBER_FIELD]);
        // 4. receipt + log
        let log = verify_receipt_log(
            &receipts_root,
            proof.tx_index,
            &proof.receipt,
            &proof.receipt_proof,
            proof.log_index,
        )?;
        if log.address != token
            || log.topics.len() != 3
            || log.topics[0] != *TRANSFER_TOPIC
            || log.data.len() != 32
        {
            return Err(Error::NotTransfer(proof.log_index));
        }
        let mut from = [0u8; 20];
        from.copy_from_slice(&log.topics[1][12..]);
        let mut to = [0u8; 20];
        to.copy_from_slice(&log.topics[2][12..]);
        let amount = u256_to_u128(&log.data).ok_or(Error::NotTransfer(proof.log_index))?;
        if proof.finalized.slot > self.last_finalized_slot {
            self.last_finalized_slot = proof.finalized.slot;
        }
        Ok(VerifiedEth {
            finalized_slot: proof.finalized.slot,
            block_number,
            receipts_root,
            token,
            from,
            to,
            amount,
            participation,
        })
    }

    /// Step 1: committee membership, participation and the BLS aggregate.
    pub fn verify_sync_aggregate(&self, proof: &EthDepositProof) -> Result<u32, Error> {
        let n = self.committee_size as usize;
        if proof.committee_pubkeys.len() != n {
            return Err(Error::Malformed("committee size"));
        }
        if proof.sync_committee_bits.len() != n.div_ceil(8) {
            return Err(Error::Malformed("bitfield length"));
        }
        if proof.sync_committee_signature.len() != 96
            || proof.committee_aggregate_pubkey.len() != 48
        {
            return Err(Error::Malformed("signature or aggregate pubkey length"));
        }
        let root = committee_root(
            &proof.committee_pubkeys,
            &proof.committee_aggregate_pubkey,
            n,
        )
        .ok_or(Error::Malformed("pubkey length"))?;
        if root != self.committee_root {
            return Err(Error::CommitteeMismatch);
        }
        let mut participants: Vec<PublicKey> = Vec::new();
        for (i, pk) in proof.committee_pubkeys.iter().enumerate() {
            if proof.sync_committee_bits[i / 8] & (1 << (i % 8)) != 0 {
                participants
                    .push(PublicKey::from_bytes(pk).map_err(|_| Error::Malformed("pubkey"))?);
            }
        }
        let count = participants.len() as u32;
        if (count as u64) * 3 < (n as u64) * 2 {
            return Err(Error::Participation(count, n as u32));
        }
        let signing_root = sha256_pair(&proof.attested.hash_tree_root(), &self.domain());
        let sig = Signature::from_bytes(&proof.sync_committee_signature)
            .map_err(|_| Error::BadSignature)?;
        let refs: Vec<&PublicKey> = participants.iter().collect();
        if sig.fast_aggregate_verify(true, &signing_root, DST, &refs) != BLST_ERROR::BLST_SUCCESS {
            return Err(Error::BadSignature);
        }
        Ok(count)
    }

    /// Step 5: accept the next committee proven under a finalized state.
    pub fn apply_committee_update(
        &mut self,
        finalized_state_root: &[u8; 32],
        finalized_slot: u64,
        next_pubkeys: &[Vec<u8>],
        next_aggregate: &[u8],
        branch: &[[u8; 32]],
    ) -> Result<(), Error> {
        let root = committee_root(next_pubkeys, next_aggregate, self.committee_size as usize)
            .ok_or(Error::Malformed("pubkey length"))?;
        if !verify_branch(
            &root,
            branch,
            self.next_sync_committee_gindex,
            finalized_state_root,
        ) {
            return Err(Error::CommitteeBranch);
        }
        let period = Self::period_of_slot(finalized_slot);
        if period != self.period {
            return Err(Error::WrongPeriod(period, self.period));
        }
        self.next_committee_root = Some(root);
        Ok(())
    }

    /// Move to the next period once a finalized header from it is seen.
    pub fn advance_period(&mut self, finalized_slot: u64) -> Result<(), Error> {
        let period = Self::period_of_slot(finalized_slot);
        if period != self.period + 1 {
            return Err(Error::WrongPeriod(period, self.period));
        }
        let next = self
            .next_committee_root
            .take()
            .ok_or(Error::Malformed("no next committee"))?;
        self.committee_root = next;
        self.period = period;
        Ok(())
    }
}

// ---------------- SSZ helpers ----------------

pub fn sha256_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    h.finalize().into()
}

pub fn u64_root(v: u64) -> [u8; 32] {
    let mut r = [0u8; 32];
    r[..8].copy_from_slice(&v.to_le_bytes());
    r
}

fn u64_from_root(r: &[u8; 32]) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&r[..8]);
    u64::from_le_bytes(b)
}

/// Merkleize `leaves` padded with zero chunks to `pad_to` (a power of two).
pub fn merkleize(leaves: &[[u8; 32]], pad_to: usize) -> [u8; 32] {
    let mut layer: Vec<[u8; 32]> = leaves.to_vec();
    layer.resize(pad_to.max(1).next_power_of_two(), [0u8; 32]);
    while layer.len() > 1 {
        layer = layer.chunks(2).map(|c| sha256_pair(&c[0], &c[1])).collect();
    }
    layer[0]
}

/// hash_tree_root of a 48-byte BLS pubkey (two 32-byte chunks).
fn pubkey_root(pk: &[u8]) -> Option<[u8; 32]> {
    if pk.len() != 48 {
        return None;
    }
    let mut a = [0u8; 32];
    a.copy_from_slice(&pk[..32]);
    let mut b = [0u8; 32];
    b[..16].copy_from_slice(&pk[32..]);
    Some(sha256_pair(&a, &b))
}

/// hash_tree_root(SyncCommittee) = merkleize([root(pubkeys), root(aggregate)]).
pub fn committee_root(pubkeys: &[Vec<u8>], aggregate: &[u8], size: usize) -> Option<[u8; 32]> {
    if pubkeys.len() != size {
        return None;
    }
    let roots: Vec<[u8; 32]> = pubkeys
        .iter()
        .map(|p| pubkey_root(p))
        .collect::<Option<_>>()?;
    let pubkeys_root = merkleize(&roots, size);
    let agg = pubkey_root(aggregate)?;
    Some(sha256_pair(&pubkeys_root, &agg))
}

/// Verify `leaf` at generalized index `gindex` under `root` using `branch`.
pub fn verify_branch(leaf: &[u8; 32], branch: &[[u8; 32]], gindex: u64, root: &[u8; 32]) -> bool {
    let depth = 63 - gindex.leading_zeros() as usize;
    if branch.len() != depth {
        return false;
    }
    let mut node = *leaf;
    let mut index = gindex;
    for sibling in branch {
        node = if index & 1 == 1 {
            sha256_pair(sibling, &node)
        } else {
            sha256_pair(&node, sibling)
        };
        index >>= 1;
    }
    node == *root
}

// ---------------- receipts ----------------

pub static TRANSFER_TOPIC: std::sync::LazyLock<[u8; 32]> =
    std::sync::LazyLock::new(|| keccak256(b"Transfer(address,address,uint256)").0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    pub address: [u8; 20],
    pub topics: Vec<[u8; 32]>,
    pub data: Vec<u8>,
}

/// Prove `receipt` is the receipt at `tx_index` under `receipts_root` and
/// return its log at `log_index`.
pub fn verify_receipt_log(
    receipts_root: &[u8; 32],
    tx_index: u64,
    receipt: &[u8],
    proof: &[Vec<u8>],
    log_index: u32,
) -> Result<Log, Error> {
    let key = alloy_rlp::encode(tx_index);
    let nodes: Vec<Bytes> = proof.iter().map(|n| Bytes::from(n.clone())).collect();
    verify_proof(
        B256::from(*receipts_root),
        Nibbles::unpack(&key),
        Some(receipt.to_vec()),
        nodes.iter(),
    )
    .map_err(|_| Error::ReceiptProof)?;
    let logs = decode_receipt_logs(receipt).ok_or(Error::ReceiptDecode)?;
    logs.into_iter()
        .nth(log_index as usize)
        .ok_or(Error::NotTransfer(log_index))
}

/// Decode the logs of a legacy or typed (EIP-2718) receipt.
pub fn decode_receipt_logs(receipt: &[u8]) -> Option<Vec<Log>> {
    let mut buf: &[u8] = receipt;
    if let Some(first) = buf.first() {
        if *first < 0x80 {
            buf = &buf[1..];
        }
    }
    let outer = RlpHeader::decode(&mut buf).ok()?;
    if !outer.list {
        return None;
    }
    let mut body = &buf[..outer.payload_length];
    // status, cumulative gas, bloom
    for _ in 0..3 {
        skip_item(&mut body)?;
    }
    let logs_hdr = RlpHeader::decode(&mut body).ok()?;
    if !logs_hdr.list {
        return None;
    }
    let mut logs_buf = &body[..logs_hdr.payload_length];
    let mut logs = Vec::new();
    while !logs_buf.is_empty() {
        let lh = RlpHeader::decode(&mut logs_buf).ok()?;
        if !lh.list {
            return None;
        }
        let mut item = &logs_buf[..lh.payload_length];
        logs_buf = &logs_buf[lh.payload_length..];
        let addr = take_string(&mut item)?;
        if addr.len() != 20 {
            return None;
        }
        let th = RlpHeader::decode(&mut item).ok()?;
        if !th.list {
            return None;
        }
        let mut topics_buf = &item[..th.payload_length];
        item = &item[th.payload_length..];
        let mut topics = Vec::new();
        while !topics_buf.is_empty() {
            let t = take_string(&mut topics_buf)?;
            if t.len() != 32 {
                return None;
            }
            let mut a = [0u8; 32];
            a.copy_from_slice(t);
            topics.push(a);
        }
        let data = take_string(&mut item)?.to_vec();
        let mut address = [0u8; 20];
        address.copy_from_slice(addr);
        logs.push(Log {
            address,
            topics,
            data,
        });
    }
    Some(logs)
}

fn skip_item(buf: &mut &[u8]) -> Option<()> {
    let h = RlpHeader::decode(buf).ok()?;
    if buf.len() < h.payload_length {
        return None;
    }
    *buf = &buf[h.payload_length..];
    Some(())
}

fn take_string<'a>(buf: &mut &'a [u8]) -> Option<&'a [u8]> {
    let h = RlpHeader::decode(buf).ok()?;
    if h.list || buf.len() < h.payload_length {
        return None;
    }
    let (s, rest) = buf.split_at(h.payload_length);
    *buf = rest;
    Some(s)
}

fn u256_to_u128(data: &[u8]) -> Option<u128> {
    if data.len() != 32 || data[..16].iter().any(|b| *b != 0) {
        return None;
    }
    let mut b = [0u8; 16];
    b.copy_from_slice(&data[16..]);
    Some(u128::from_be_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address as EthAddress, Bloom, Log as ALog, LogData, B256};
    use alloy_rlp::Encodable;
    use alloy_trie::{proof::ProofRetainer, HashBuilder};
    use blst::min_pk::{AggregatePublicKey, SecretKey};

    const N: usize = 8;

    fn keys() -> Vec<SecretKey> {
        (0..N as u8)
            .map(|i| SecretKey::key_gen(&[i + 1; 32], &[]).unwrap())
            .collect()
    }

    fn header(slot: u64, state_root: [u8; 32], body_root: [u8; 32]) -> BeaconHeader {
        BeaconHeader {
            slot,
            proposer_index: 1,
            parent_root: [1u8; 32],
            state_root,
            body_root,
        }
    }

    /// Build a branch for `leaf` at `gindex` under a root of otherwise-zero
    /// siblings; returns (branch, root).
    fn branch_for(leaf: [u8; 32], gindex: u64) -> (Vec<[u8; 32]>, [u8; 32]) {
        let depth = 63 - gindex.leading_zeros() as usize;
        let mut branch = Vec::new();
        let mut node = leaf;
        let mut idx = gindex;
        for i in 0..depth {
            let sibling = [i as u8 + 10; 32];
            branch.push(sibling);
            node = if idx & 1 == 1 {
                sha256_pair(&sibling, &node)
            } else {
                sha256_pair(&node, &sibling)
            };
            idx >>= 1;
        }
        (branch, node)
    }

    fn receipt_with_transfer(
        token: [u8; 20],
        from: [u8; 20],
        to: [u8; 20],
        amount: u128,
    ) -> Vec<u8> {
        let mut data = [0u8; 32];
        data[16..].copy_from_slice(&amount.to_be_bytes());
        let mut t1 = [0u8; 32];
        t1[12..].copy_from_slice(&from);
        let mut t2 = [0u8; 32];
        t2[12..].copy_from_slice(&to);
        let log = ALog {
            address: EthAddress::from(token),
            data: LogData::new_unchecked(
                vec![B256::from(*TRANSFER_TOPIC), B256::from(t1), B256::from(t2)],
                data.to_vec().into(),
            ),
        };
        let noise = ALog {
            address: EthAddress::from([9u8; 20]),
            data: LogData::new_unchecked(vec![B256::from([3u8; 32])], vec![1, 2, 3].into()),
        };
        // legacy receipt: [status, cumulative_gas, bloom, logs]
        #[derive(alloy_rlp::RlpEncodable)]
        struct R {
            status: bool,
            gas: u64,
            bloom: Bloom,
            logs: Vec<ALog>,
        }
        let r = R {
            status: true,
            gas: 21_000,
            bloom: Bloom::default(),
            logs: vec![noise, log],
        };
        let mut out = vec![0x02u8]; // EIP-1559 typed receipt
        r.encode(&mut out);
        out
    }

    fn receipts_trie(receipts: &[Vec<u8>], prove: u64) -> ([u8; 32], Vec<Vec<u8>>) {
        let mut entries: Vec<(Nibbles, Vec<u8>)> = receipts
            .iter()
            .enumerate()
            .map(|(i, r)| (Nibbles::unpack(alloy_rlp::encode(i as u64)), r.clone()))
            .collect();
        entries.sort_by_key(|e| e.0);
        let target = Nibbles::unpack(alloy_rlp::encode(prove));
        let mut hb = HashBuilder::default().with_proof_retainer(ProofRetainer::new(vec![target]));
        for (k, v) in &entries {
            hb.add_leaf(*k, v);
        }
        let root = hb.root();
        let proof: Vec<Vec<u8>> = hb
            .take_proof_nodes()
            .matching_nodes_sorted(&target)
            .into_iter()
            .map(|(_, v)| v.to_vec())
            .collect();
        (root.0, proof)
    }

    fn make_proof(
        state: &SyncCommitteeState,
        sks: &[SecretKey],
        participating: usize,
    ) -> (EthDepositProof, [u8; 20], u128) {
        let token = [0xaa; 20];
        let to = [0xbb; 20];
        let amount = 1_234_567u128;
        let receipts = vec![
            vec![0x02, 0xc0],
            receipt_with_transfer(token, [0xcc; 20], to, amount),
            vec![0xc0],
        ];
        let (receipts_root, receipt_proof) = receipts_trie(&receipts, 1);
        let mut fields = vec![[0u8; 32]; EXECUTION_HEADER_FIELDS];
        fields[RECEIPTS_ROOT_FIELD] = receipts_root;
        fields[BLOCK_NUMBER_FIELD] = u64_root(19_000_000);
        let payload_root = merkleize(&fields, 32);
        let (execution_branch, body_root) = branch_for(payload_root, EXECUTION_PAYLOAD_GINDEX);
        let finalized = header(8_000, [5u8; 32], body_root);
        let (finality_branch, state_root) =
            branch_for(finalized.hash_tree_root(), FINALIZED_ROOT_GINDEX);
        let attested = header(8_064, state_root, [6u8; 32]);
        let signing_root = sha256_pair(&attested.hash_tree_root(), &state.domain());
        let sigs: Vec<_> = sks
            .iter()
            .take(participating)
            .map(|k| k.sign(&signing_root, DST, &[]))
            .collect();
        let refs: Vec<_> = sigs.iter().collect();
        let agg = blst::min_pk::AggregateSignature::aggregate(&refs, true)
            .unwrap()
            .to_signature();
        let mut bits = vec![0u8; N.div_ceil(8)];
        for i in 0..participating {
            bits[i / 8] |= 1 << (i % 8);
        }
        let pks: Vec<Vec<u8>> = sks
            .iter()
            .map(|k| k.sk_to_pk().to_bytes().to_vec())
            .collect();
        let pk_refs: Vec<PublicKey> = sks.iter().map(|k| k.sk_to_pk()).collect();
        let agg_pk = AggregatePublicKey::aggregate(&pk_refs.iter().collect::<Vec<_>>(), true)
            .unwrap()
            .to_public_key()
            .to_bytes()
            .to_vec();
        let proof = EthDepositProof {
            attested,
            finalized,
            finality_branch,
            sync_committee_bits: bits,
            sync_committee_signature: agg.to_bytes().to_vec(),
            committee_pubkeys: pks,
            committee_aggregate_pubkey: agg_pk,
            execution_fields: fields,
            execution_branch,
            tx_index: 1,
            log_index: 1,
            receipt: receipts[1].clone(),
            receipt_proof,
        };
        (proof, token, amount)
    }

    fn state_for(sks: &[SecretKey]) -> SyncCommitteeState {
        let pks: Vec<Vec<u8>> = sks
            .iter()
            .map(|k| k.sk_to_pk().to_bytes().to_vec())
            .collect();
        let pk_refs: Vec<PublicKey> = sks.iter().map(|k| k.sk_to_pk()).collect();
        let agg = AggregatePublicKey::aggregate(&pk_refs.iter().collect::<Vec<_>>(), true)
            .unwrap()
            .to_public_key()
            .to_bytes()
            .to_vec();
        let root = committee_root(&pks, &agg, N).unwrap();
        let mut s = SyncCommitteeState::checkpoint(0, root, [7u8; 32], [4, 0, 0, 0]);
        s.committee_size = N as u32;
        s
    }

    #[test]
    fn full_deposit_proof_verifies() {
        let sks = keys();
        let mut state = state_for(&sks);
        let (proof, token, amount) = make_proof(&state, &sks, N);
        let v = state.verify_deposit(&proof, token).unwrap();
        assert_eq!(v.amount, amount);
        assert_eq!(v.to, [0xbb; 20]);
        assert_eq!(v.block_number, 19_000_000);
        assert_eq!(v.participation, N as u32);
        assert_eq!(state.last_finalized_slot, 8_000);
        // Wrong token contract.
        assert_eq!(
            state.verify_deposit(&proof, [0x01; 20]),
            Err(Error::NotTransfer(1))
        );
    }

    #[test]
    fn participation_and_signature_are_enforced() {
        let sks = keys();
        let state = state_for(&sks);
        // 6 of 8 = 75% ok; 5 of 8 = 62.5% not ok.
        let (p6, _, _) = make_proof(&state, &sks, 6);
        assert_eq!(state.verify_sync_aggregate(&p6), Ok(6));
        let (p5, _, _) = make_proof(&state, &sks, 5);
        assert_eq!(
            state.verify_sync_aggregate(&p5),
            Err(Error::Participation(5, 8))
        );
        // Bits claim a signer that did not sign.
        let (mut bad, _, _) = make_proof(&state, &sks, 6);
        bad.sync_committee_bits[0] |= 1 << 7;
        assert_eq!(state.verify_sync_aggregate(&bad), Err(Error::BadSignature));
        // Different committee than stored.
        let other = state_for(
            &(0..N as u8)
                .map(|i| SecretKey::key_gen(&[i + 50; 32], &[]).unwrap())
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            other.verify_sync_aggregate(&p6),
            Err(Error::CommitteeMismatch)
        );
        // Tampered attested header.
        let (mut t, _, _) = make_proof(&state, &sks, N);
        t.attested.slot += 1;
        assert_eq!(state.verify_sync_aggregate(&t), Err(Error::BadSignature));
    }

    #[test]
    fn branches_and_receipt_proof_are_enforced() {
        let sks = keys();
        let mut state = state_for(&sks);
        let (proof, token, _) = make_proof(&state, &sks, N);
        let mut p = proof.clone();
        p.finality_branch[0] = [0u8; 32];
        assert_eq!(state.verify_deposit(&p, token), Err(Error::FinalityBranch));
        let mut p = proof.clone();
        p.execution_fields[RECEIPTS_ROOT_FIELD] = [1u8; 32];
        assert_eq!(state.verify_deposit(&p, token), Err(Error::ExecutionBranch));
        let mut p = proof.clone();
        p.tx_index = 2;
        assert_eq!(state.verify_deposit(&p, token), Err(Error::ReceiptProof));
        let mut p = proof.clone();
        p.log_index = 0;
        assert_eq!(state.verify_deposit(&p, token), Err(Error::NotTransfer(0)));
        let mut p = proof.clone();
        p.receipt[5] ^= 1;
        assert_eq!(state.verify_deposit(&p, token), Err(Error::ReceiptProof));
    }

    #[test]
    fn committee_rotation() {
        let sks = keys();
        let mut state = state_for(&sks);
        let next: Vec<SecretKey> = (0..N as u8)
            .map(|i| SecretKey::key_gen(&[i + 90; 32], &[]).unwrap())
            .collect();
        let pks: Vec<Vec<u8>> = next
            .iter()
            .map(|k| k.sk_to_pk().to_bytes().to_vec())
            .collect();
        let pk_refs: Vec<PublicKey> = next.iter().map(|k| k.sk_to_pk()).collect();
        let agg = AggregatePublicKey::aggregate(&pk_refs.iter().collect::<Vec<_>>(), true)
            .unwrap()
            .to_public_key()
            .to_bytes()
            .to_vec();
        let root = committee_root(&pks, &agg, N).unwrap();
        let (branch, state_root) = branch_for(root, NEXT_SYNC_COMMITTEE_GINDEX);
        assert_eq!(
            state.apply_committee_update(&[0u8; 32], 100, &pks, &agg, &branch),
            Err(Error::CommitteeBranch)
        );
        state
            .apply_committee_update(&state_root, 100, &pks, &agg, &branch)
            .unwrap();
        assert_eq!(state.advance_period(100), Err(Error::WrongPeriod(0, 0)));
        state
            .advance_period(SLOTS_PER_EPOCH * EPOCHS_PER_SYNC_COMMITTEE_PERIOD + 1)
            .unwrap();
        assert_eq!(state.period, 1);
        assert_eq!(state.committee_root, root);
        // New committee signs now.
        let (proof, token, _) = make_proof(&state, &next, N);
        assert!(state.verify_deposit(&proof, token).is_ok());
    }

    #[test]
    fn ssz_helpers_match_known_shapes() {
        assert!(verify_branch(&[1u8; 32], &[], 1, &[1u8; 32]));
        let (b, r) = branch_for([2u8; 32], 6);
        assert!(verify_branch(&[2u8; 32], &b, 6, &r));
        assert!(!verify_branch(&[3u8; 32], &b, 6, &r));
        assert_eq!(merkleize(&[[0u8; 32]], 1), [0u8; 32]);
        assert_eq!(u64_from_root(&u64_root(77)), 77);
    }
}
