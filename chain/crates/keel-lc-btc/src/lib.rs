//! Bitcoin SPV verification for deposit observations.
//!
//! What is verified, and what is not:
//! - Every header decodes, satisfies its own proof of work (`bits`), and
//!   links to the previous one by `prev_blockhash`.
//! - [`HeaderChain`] tracks the best chain from a trusted checkpoint and
//!   enforces the retarget rule at every 2016-block boundary on networks
//!   that retarget (mainnet, testnet; regtest never does). Blocks with
//!   `allow_min_difficulty_blocks` (testnet) are accepted at the minimum
//!   target only when the timestamp gap exceeds twice the target spacing,
//!   as Core does.
//! - The partial merkle tree proves the txid sits at `tx_index` under the
//!   containing header's `merkle_root`.
//! - Depth: at least `required_depth` headers build on the containing one.
//! - NOT verified: the transaction body itself (amount and output script).
//!   The observer quorum attests those, and the VM credits only when both
//!   the proof and the quorum agree (docs/plan.md §4).
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub use bitcoin::Network;

use bitcoin::{
    block::Header,
    consensus::{deserialize, serialize},
    hashes::Hash,
    merkle_tree::PartialMerkleTree,
    params::Params,
    pow::{CompactTarget, Target, Work},
    BlockHash, Txid,
};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const HEADER_BYTES: usize = 80;
/// Headers remembered by height for inclusion checks (one retarget period).
pub const WINDOW: u64 = 2016;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("header {0} is not 80 bytes")]
    BadHeaderLength(usize),
    #[error("header {0} failed to decode")]
    BadHeader(usize),
    #[error("header {0} fails its proof of work")]
    BadPow(usize),
    #[error("header {0} does not link to header {1}")]
    BrokenLink(usize, usize),
    #[error("header {index} has wrong target: expected {expected:#x}, got {got:#x}")]
    WrongTarget {
        index: usize,
        expected: u32,
        got: u32,
    },
    #[error("merkle proof is malformed")]
    BadMerkleProof,
    #[error("merkle root does not match the header")]
    MerkleRootMismatch,
    #[error("txid not proven at index {0}")]
    TxNotProven(u32),
    #[error("need {required} confirmations, have {have}")]
    NotDeep { required: u32, have: u32 },
    #[error("no headers")]
    Empty,
    #[error("header does not extend the known tip")]
    DoesNotExtendTip,
    #[error("block {0} at height {1} unknown to the header chain")]
    UnknownBlock(String, u64),
    #[error("timestamp not after median of the last 11 blocks")]
    BadTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBtc {
    pub block_hash: [u8; 32],
    pub merkle_root: [u8; 32],
    pub txid: [u8; 32],
    pub depth: u32,
    /// Cumulative work of the supplied headers, little-endian 256-bit.
    pub cumulative_work: [u8; 32],
}

fn decode_headers(headers: &[Vec<u8>]) -> Result<Vec<Header>, Error> {
    if headers.is_empty() {
        return Err(Error::Empty);
    }
    headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            if h.len() != HEADER_BYTES {
                return Err(Error::BadHeaderLength(i));
            }
            deserialize::<Header>(h).map_err(|_| Error::BadHeader(i))
        })
        .collect()
}

/// Check PoW and linkage of a header list; returns cumulative work.
fn check_pow_and_links(headers: &[Header]) -> Result<Work, Error> {
    let mut work: Option<Work> = None;
    for (i, h) in headers.iter().enumerate() {
        h.validate_pow(h.target()).map_err(|_| Error::BadPow(i))?;
        if i > 0 && h.prev_blockhash != headers[i - 1].block_hash() {
            return Err(Error::BrokenLink(i, i - 1));
        }
        work = Some(match work {
            None => h.work(),
            Some(w) => w + h.work(),
        });
    }
    work.ok_or(Error::Empty)
}

/// Pure SPV check of one deposit (see the crate docs for what this covers).
pub fn verify_deposit(
    headers: &[Vec<u8>],
    merkle_proof: &[u8],
    tx_index: u32,
    txid: [u8; 32],
    required_depth: u32,
    _network: Network,
) -> Result<VerifiedBtc, Error> {
    let decoded = decode_headers(headers)?;
    let work = check_pow_and_links(&decoded)?;
    let depth = (decoded.len() - 1) as u32;
    if depth < required_depth {
        return Err(Error::NotDeep {
            required: required_depth,
            have: depth,
        });
    }
    let pmt: PartialMerkleTree = deserialize(merkle_proof).map_err(|_| Error::BadMerkleProof)?;
    let mut matches = Vec::new();
    let mut indexes = Vec::new();
    let root = pmt
        .extract_matches(&mut matches, &mut indexes)
        .map_err(|_| Error::BadMerkleProof)?;
    if root != decoded[0].merkle_root {
        return Err(Error::MerkleRootMismatch);
    }
    let want = Txid::from_byte_array(txid);
    let proven = matches
        .iter()
        .zip(indexes.iter())
        .any(|(t, i)| *t == want && *i == tx_index);
    if !proven {
        return Err(Error::TxNotProven(tx_index));
    }
    Ok(VerifiedBtc {
        block_hash: decoded[0].block_hash().to_byte_array(),
        merkle_root: decoded[0].merkle_root.to_byte_array(),
        txid,
        depth,
        cumulative_work: work.to_le_bytes(),
    })
}

/// The header chain a node tracks from a trusted checkpoint. Stored by the
/// VM per chain (borsh) so every validator agrees on the tip.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct HeaderChain {
    pub network: u8,
    pub checkpoint_height: u64,
    pub tip_height: u64,
    pub tip_hash: [u8; 32],
    pub tip_bits: u32,
    pub tip_time: u32,
    /// Timestamp of the first block of the current retarget period.
    pub period_start_time: u32,
    /// Cumulative work since the checkpoint, little-endian.
    pub work: [u8; 32],
    /// height -> block hash for the last [`WINDOW`] blocks.
    pub recent: BTreeMap<u64, [u8; 32]>,
    /// Timestamps of the last 11 blocks, oldest first (median-time-past).
    pub recent_times: Vec<u32>,
}

fn network_code(n: Network) -> u8 {
    match n {
        Network::Bitcoin => 0,
        Network::Testnet => 1,
        Network::Signet => 2,
        Network::Regtest => 3,
        _ => 255,
    }
}

fn network_from(code: u8) -> Network {
    match code {
        0 => Network::Bitcoin,
        1 => Network::Testnet,
        2 => Network::Signet,
        _ => Network::Regtest,
    }
}

impl HeaderChain {
    /// Start from a trusted header (the checkpoint) at `height`.
    /// `period_start_time` is the timestamp of block `height - height % 2016`.
    pub fn from_checkpoint(
        network: Network,
        height: u64,
        header: &[u8],
        period_start_time: u32,
    ) -> Result<Self, Error> {
        let h = decode_headers(&[header.to_vec()])?.remove(0);
        h.validate_pow(h.target()).map_err(|_| Error::BadPow(0))?;
        let mut recent = BTreeMap::new();
        recent.insert(height, h.block_hash().to_byte_array());
        Ok(Self {
            network: network_code(network),
            checkpoint_height: height,
            tip_height: height,
            tip_hash: h.block_hash().to_byte_array(),
            tip_bits: h.bits.to_consensus(),
            tip_time: h.time,
            period_start_time,
            work: h.work().to_le_bytes(),
            recent,
            recent_times: vec![h.time],
        })
    }

    pub fn network(&self) -> Network {
        network_from(self.network)
    }

    pub fn contains(&self, height: u64, hash: &[u8; 32]) -> bool {
        self.recent.get(&height) == Some(hash)
    }

    pub fn height_of(&self, hash: &[u8; 32]) -> Option<u64> {
        self.recent
            .iter()
            .find(|(_, h)| *h == hash)
            .map(|(h, _)| *h)
    }

    /// Depth of `hash` below the tip, if known.
    pub fn depth_of(&self, hash: &[u8; 32]) -> Option<u32> {
        self.height_of(hash).map(|h| (self.tip_height - h) as u32)
    }

    fn expected_bits(&self, next_height: u64, next_time: u32, params: &Params) -> u32 {
        let interval = params.difficulty_adjustment_interval();
        if !params.no_pow_retargeting && next_height.is_multiple_of(interval) {
            let timespan = self.tip_time.saturating_sub(self.period_start_time) as u64;
            return CompactTarget::from_next_work_required(
                CompactTarget::from_consensus(self.tip_bits),
                timespan,
                params,
            )
            .to_consensus();
        }
        if params.allow_min_difficulty_blocks
            && (next_time as u64) > self.tip_time as u64 + params.pow_target_spacing * 2
        {
            return params
                .max_attainable_target
                .to_compact_lossy()
                .to_consensus();
        }
        self.tip_bits
    }

    fn median_time_past(&self) -> u32 {
        let mut t = self.recent_times.clone();
        t.sort_unstable();
        t[t.len() / 2]
    }

    /// Append headers that build on the current tip. All-or-nothing.
    pub fn extend(&mut self, headers: &[Vec<u8>]) -> Result<u64, Error> {
        let decoded = decode_headers(headers)?;
        if decoded[0].prev_blockhash.to_byte_array() != self.tip_hash {
            return Err(Error::DoesNotExtendTip);
        }
        let params = Params::new(self.network());
        let mut next = self.clone();
        for (i, h) in decoded.iter().enumerate() {
            let height = next.tip_height + 1;
            let expected = next.expected_bits(height, h.time, &params);
            if h.bits.to_consensus() != expected {
                return Err(Error::WrongTarget {
                    index: i,
                    expected,
                    got: h.bits.to_consensus(),
                });
            }
            h.validate_pow(Target::from_compact(h.bits))
                .map_err(|_| Error::BadPow(i))?;
            if h.prev_blockhash.to_byte_array() != next.tip_hash {
                return Err(Error::BrokenLink(i, i.saturating_sub(1)));
            }
            if h.time <= next.median_time_past() && next.recent_times.len() >= 11 {
                return Err(Error::BadTimestamp);
            }
            let interval = params.difficulty_adjustment_interval();
            if height.is_multiple_of(interval) {
                next.period_start_time = h.time;
            }
            next.work = (Work::from_le_bytes(next.work) + h.work()).to_le_bytes();
            next.tip_height = height;
            next.tip_hash = h.block_hash().to_byte_array();
            next.tip_bits = h.bits.to_consensus();
            next.tip_time = h.time;
            next.recent.insert(height, next.tip_hash);
            next.recent_times.push(h.time);
            if next.recent_times.len() > 11 {
                next.recent_times.remove(0);
            }
            while next.recent.len() as u64 > WINDOW {
                let first = *next.recent.keys().next().expect("non-empty");
                next.recent.remove(&first);
            }
        }
        *self = next;
        Ok(self.tip_height)
    }

    /// Verify a deposit against this chain: the containing block must be
    /// known and at least `required_depth` below the tip. Headers past the
    /// tip are folded in first when they extend it.
    pub fn verify_deposit(
        &mut self,
        headers: &[Vec<u8>],
        merkle_proof: &[u8],
        tx_index: u32,
        txid: [u8; 32],
        required_depth: u32,
    ) -> Result<VerifiedBtc, Error> {
        let decoded = decode_headers(headers)?;
        let first_hash = decoded[0].block_hash().to_byte_array();
        // Fold in any suffix of the supplied headers that extends our tip.
        if let Some(pos) = decoded
            .iter()
            .position(|h| h.prev_blockhash.to_byte_array() == self.tip_hash)
        {
            let _ = self.extend(&headers[pos..]);
        }
        let height = self
            .height_of(&first_hash)
            .ok_or_else(|| Error::UnknownBlock(decoded[0].block_hash().to_string(), 0))?;
        let have = (self.tip_height - height) as u32;
        if have < required_depth {
            return Err(Error::NotDeep {
                required: required_depth,
                have,
            });
        }
        // Pure check on the supplied headers (with depth 0 since the chain
        // already supplies the depth), then report the chain's depth.
        let mut v = verify_deposit(headers, merkle_proof, tx_index, txid, 0, self.network())?;
        v.depth = have;
        Ok(v)
    }
}

/// Serialize a header (helper for observers and tests).
pub fn encode_header(h: &Header) -> Vec<u8> {
    serialize(h)
}

pub fn block_hash_of(header: &[u8]) -> Option<[u8; 32]> {
    deserialize::<Header>(header)
        .ok()
        .map(|h| h.block_hash().to_byte_array())
}

pub fn block_hash_hex(hash: &[u8; 32]) -> String {
    BlockHash::from_byte_array(*hash).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        block::Version, blockdata::constants::genesis_block, hashes::sha256d, TxMerkleNode,
    };

    fn txid(n: u8) -> Txid {
        Txid::from_byte_array(sha256d::Hash::hash(&[n]).to_byte_array())
    }

    /// Mine a regtest header on top of `prev` with the given merkle root.
    fn mine(prev: &Header, merkle_root: TxMerkleNode, time: u32) -> Header {
        let mut h = Header {
            version: Version::TWO,
            prev_blockhash: prev.block_hash(),
            merkle_root,
            time,
            bits: prev.bits,
            nonce: 0,
        };
        while h.validate_pow(h.target()).is_err() {
            h.nonce += 1;
        }
        h
    }

    fn chain(n: usize, txids: &[Txid]) -> (Vec<Header>, PartialMerkleTree) {
        let genesis = genesis_block(Network::Regtest).header;
        let matches: Vec<bool> = (0..txids.len()).map(|i| i == 1).collect();
        let pmt = PartialMerkleTree::from_txids(txids, &matches);
        let mut m = Vec::new();
        let mut idx = Vec::new();
        let root = pmt.extract_matches(&mut m, &mut idx).unwrap();
        let mut headers = vec![genesis];
        for i in 0..n {
            let prev = *headers.last().unwrap();
            let mr = if i == 0 {
                root
            } else {
                TxMerkleNode::from_byte_array(sha256d::Hash::hash(&[i as u8]).to_byte_array())
            };
            headers.push(mine(&prev, mr, prev.time + 600));
        }
        (headers, pmt)
    }

    #[test]
    fn spv_proof_verifies_and_rejects_tampering() {
        let txids = vec![txid(1), txid(2), txid(3)];
        let (headers, pmt) = chain(4, &txids);
        let bytes: Vec<Vec<u8>> = headers[1..].iter().map(encode_header).collect();
        let proof = serialize(&pmt);
        let v = verify_deposit(
            &bytes,
            &proof,
            1,
            txid(2).to_byte_array(),
            3,
            Network::Regtest,
        )
        .unwrap();
        assert_eq!(v.depth, 3);
        assert_eq!(v.block_hash, headers[1].block_hash().to_byte_array());
        // Wrong index / wrong txid / not deep enough / broken link / bad pow.
        assert_eq!(
            verify_deposit(
                &bytes,
                &proof,
                0,
                txid(2).to_byte_array(),
                3,
                Network::Regtest
            ),
            Err(Error::TxNotProven(0))
        );
        assert!(matches!(
            verify_deposit(
                &bytes,
                &proof,
                1,
                txid(9).to_byte_array(),
                3,
                Network::Regtest
            ),
            Err(Error::TxNotProven(1))
        ));
        assert_eq!(
            verify_deposit(
                &bytes[..2],
                &proof,
                1,
                txid(2).to_byte_array(),
                3,
                Network::Regtest
            ),
            Err(Error::NotDeep {
                required: 3,
                have: 1
            })
        );
        let mut broken = bytes.clone();
        broken.swap(1, 2);
        assert!(matches!(
            verify_deposit(
                &broken,
                &proof,
                1,
                txid(2).to_byte_array(),
                1,
                Network::Regtest
            ),
            Err(Error::BrokenLink(..))
        ));
        let mut bad = bytes.clone();
        bad[2][76] ^= 0xff; // nonce
        assert!(matches!(
            verify_deposit(
                &bad,
                &proof,
                1,
                txid(2).to_byte_array(),
                1,
                Network::Regtest
            ),
            Err(Error::BadPow(2)) | Err(Error::BrokenLink(..))
        ));
        // Proof against the wrong header (merkle root mismatch).
        assert_eq!(
            verify_deposit(
                &bytes[1..],
                &proof,
                1,
                txid(2).to_byte_array(),
                1,
                Network::Regtest
            ),
            Err(Error::MerkleRootMismatch)
        );
    }

    #[test]
    fn header_chain_extends_tracks_depth_and_refuses_forks() {
        let txids = vec![txid(1), txid(2)];
        let (headers, pmt) = chain(6, &txids);
        let genesis = headers[0];
        let mut hc = HeaderChain::from_checkpoint(
            Network::Regtest,
            0,
            &encode_header(&genesis),
            genesis.time,
        )
        .unwrap();
        let all: Vec<Vec<u8>> = headers[1..].iter().map(encode_header).collect();
        assert_eq!(hc.extend(&all[..3]).unwrap(), 3);
        assert_eq!(hc.tip_hash, headers[3].block_hash().to_byte_array());
        assert!(hc.contains(1, &headers[1].block_hash().to_byte_array()));
        // A header that does not build on the tip is refused, all-or-nothing.
        let before = hc.clone();
        assert_eq!(hc.extend(&all[4..]), Err(Error::DoesNotExtendTip));
        assert_eq!(hc, before);
        // Deposit in block 1 with depth 2 known; require 5 -> not deep; fold in the rest -> ok.
        let proof = serialize(&pmt);
        let t = txid(2).to_byte_array();
        assert_eq!(
            hc.verify_deposit(&all[..3], &proof, 1, t, 5),
            Err(Error::NotDeep {
                required: 5,
                have: 2
            })
        );
        let v = hc.verify_deposit(&all, &proof, 1, t, 5).unwrap();
        assert_eq!(v.depth, 5);
        assert_eq!(hc.tip_height, 6);
        // Unknown block.
        let stranger = mine(&genesis, TxMerkleNode::all_zeros(), genesis.time + 1);
        assert!(matches!(
            hc.verify_deposit(&[encode_header(&stranger)], &proof, 1, t, 0),
            Err(Error::UnknownBlock(..))
        ));
        // Work accumulates.
        assert_ne!(hc.work, [0u8; 32]);
        assert_eq!(hc.recent.len(), 7);
    }

    #[test]
    fn mainnet_retarget_rule_is_enforced() {
        // Synthetic: a regtest-difficulty chain labelled mainnet would fail
        // its target check at the first block after the checkpoint because
        // mainnet's expected bits carry over from the checkpoint (regtest
        // bits differ). We check the error surface rather than mining
        // mainnet difficulty.
        let genesis = genesis_block(Network::Regtest).header;
        let mut hc = HeaderChain::from_checkpoint(
            Network::Bitcoin,
            2015,
            &encode_header(&genesis),
            genesis.time,
        )
        .unwrap();
        let next = mine(&genesis, TxMerkleNode::all_zeros(), genesis.time + 600);
        // Height 2016 is a retarget boundary: expected bits are recomputed
        // from the (clamped) timespan; regtest bits are already at the
        // max-attainable target, so the retarget returns the mainnet limit,
        // which differs from regtest bits.
        let err = hc.extend(&[encode_header(&next)]).unwrap_err();
        assert!(matches!(err, Error::WrongTarget { .. }), "{err:?}");
        assert_eq!(hc.tip_height, 2015);
    }
}
