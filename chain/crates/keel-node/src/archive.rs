//! Random access to the node's own history for the RPC (explorers, the
//! indexer). Append-only files under the storage dir, cut into segments
//! at every snapshot (`machine.rs` owns the writes):
//!
//! - `journal-<from>.bin`: `u32 len | borsh JournalEntry` per block —
//!   height, timestamp, payload — for the blocks from height `from` up to
//!   the next segment's start.
//! - `receipts-<from>.jsonl`: one JSON line per receipt, `{height, receipt}`,
//!   appended per block in order, same segmentation.
//! - `hashes.bin` (owned here): 40 bytes per block, `u64 height | [u8;32]
//!   state hash`, so the hash after every block survives restarts.
//!
//! A pruning node deletes whole segments below its retention line; the
//! archive then serves a suffix of the chain and `oldest()` says where it
//! starts. At startup the journals are scanned once to build height → byte
//! range indexes in memory (a few MB per million blocks); after that each
//! read is one seek. Nothing here is on the consensus path.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use anyhow::Context as _;
use borsh::BorshDeserialize;
use keel_actions::{decode_payload, SignedAction};
use keel_vm::receipt::Receipt;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

#[derive(BorshDeserialize)]
struct JournalEntry {
    height: u64,
    timestamp: u64,
    payload: Vec<u8>,
}

pub fn journal_path(dir: &Path, from: u64) -> PathBuf {
    dir.join(format!("journal-{from}.bin"))
}

pub fn receipts_path(dir: &Path, from: u64) -> PathBuf {
    dir.join(format!("receipts-{from}.jsonl"))
}

/// Segment start heights present on disk, ascending.
pub fn segments(dir: &Path) -> anyhow::Result<Vec<u64>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name().to_string_lossy().to_string();
        if let Some(h) = name
            .strip_prefix("journal-")
            .and_then(|s| s.strip_suffix(".bin"))
        {
            if let Ok(h) = h.parse::<u64>() {
                out.push(h);
            }
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// Storage written before segmentation had one `journal.bin` and one
/// `receipts.jsonl`; they become the segment starting at height 1.
pub fn migrate_legacy(dir: &Path) -> anyhow::Result<()> {
    let legacy = dir.join("journal.bin");
    if legacy.exists() && !journal_path(dir, 1).exists() {
        fs::rename(&legacy, journal_path(dir, 1)).context("rename journal.bin")?;
        let receipts = dir.join("receipts.jsonl");
        if receipts.exists() {
            fs::rename(&receipts, receipts_path(dir, 1)).context("rename receipts.jsonl")?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct BlockMeta {
    pub height: u64,
    pub timestamp: u64,
    pub state_hash: Option<[u8; 32]>,
    pub tx_count: u32,
}

pub struct Archive {
    dir: PathBuf,
    /// height -> (segment, offset of the entry body, length)
    journal_index: BTreeMap<u64, (u64, u64, u32)>,
    /// height -> (segment, start, end) byte range of that block's receipt lines
    receipts_index: BTreeMap<u64, (u64, u64, u64)>,
    hashes: BTreeMap<u64, [u8; 32]>,
    hashes_file: File,
    /// Bytes written so far per receipts segment.
    receipts_len: BTreeMap<u64, u64>,
    /// Cached tx counts from the index scan (payload decode is lazy).
    tx_counts: BTreeMap<u64, u32>,
}

impl Archive {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        migrate_legacy(dir)?;
        let mut journal_index = BTreeMap::new();
        let mut tx_counts = BTreeMap::new();
        let mut receipts_index = BTreeMap::new();
        let mut receipts_len = BTreeMap::new();
        for seg in segments(dir)? {
            if let Ok(mut f) = File::open(journal_path(dir, seg)) {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                let mut pos = 0usize;
                while pos + 4 <= buf.len() {
                    let len =
                        u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
                            as usize;
                    pos += 4;
                    if pos + len > buf.len() {
                        break;
                    }
                    if let Ok(e) = JournalEntry::try_from_slice(&buf[pos..pos + len]) {
                        journal_index.insert(e.height, (seg, pos as u64, len as u32));
                        tx_counts.insert(
                            e.height,
                            decode_payload(&e.payload)
                                .map(|a| a.len() as u32)
                                .unwrap_or(0),
                        );
                    }
                    pos += len;
                }
            }
            let mut offset = 0u64;
            if let Ok(mut f) = File::open(receipts_path(dir, seg)) {
                let mut buf = String::new();
                f.read_to_string(&mut buf)?;
                for line in buf.split_inclusive('\n') {
                    let len = line.len() as u64;
                    if let Some(h) = height_of_line(line) {
                        receipts_index
                            .entry(h)
                            .and_modify(|(_, _, end)| *end = offset + len)
                            .or_insert((seg, offset, offset + len));
                    }
                    offset += len;
                }
            }
            receipts_len.insert(seg, offset);
        }
        let mut hashes = BTreeMap::new();
        let mut hashes_file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(dir.join("hashes.bin"))?;
        {
            let mut buf = Vec::new();
            hashes_file.seek(SeekFrom::Start(0))?;
            hashes_file.read_to_end(&mut buf)?;
            for rec in buf.as_chunks::<40>().0 {
                let mut h8 = [0u8; 8];
                h8.copy_from_slice(&rec[..8]);
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&rec[8..]);
                hashes.insert(u64::from_le_bytes(h8), hash);
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            journal_index,
            receipts_index,
            hashes,
            hashes_file,
            receipts_len,
            tx_counts,
        })
    }

    /// Record a block that `machine.rs` just journaled into segment
    /// `journal_seg`; `receipt_bytes` is what was appended to the receipts
    /// file of `receipt_seg` for it.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        height: u64,
        tx_count: u32,
        state_hash: [u8; 32],
        journal_seg: u64,
        journal_body_offset: u64,
        journal_body_len: u32,
        receipt_seg: u64,
        receipt_bytes: u64,
    ) -> anyhow::Result<()> {
        self.journal_index
            .insert(height, (journal_seg, journal_body_offset, journal_body_len));
        self.tx_counts.insert(height, tx_count);
        let len = self.receipts_len.entry(receipt_seg).or_insert(0);
        if receipt_bytes > 0 {
            self.receipts_index
                .insert(height, (receipt_seg, *len, *len + receipt_bytes));
        }
        *len += receipt_bytes;
        let mut rec = [0u8; 40];
        rec[..8].copy_from_slice(&height.to_le_bytes());
        rec[8..].copy_from_slice(&state_hash);
        self.hashes_file
            .write_all(&rec)
            .context("hashes.bin append")?;
        self.hashes_file.flush()?;
        self.hashes.insert(height, state_hash);
        Ok(())
    }

    /// Forget the blocks of segments that `machine.rs` deleted. Hashes are
    /// kept: they are 40 bytes per block and let the RPC answer
    /// `/v1/blocks/{h}` state hashes for pruned heights.
    pub fn prune_segments(&mut self, removed: &[u64]) {
        self.journal_index
            .retain(|_, (seg, _, _)| !removed.contains(seg));
        self.receipts_index
            .retain(|_, (seg, _, _)| !removed.contains(seg));
        for seg in removed {
            self.receipts_len.remove(seg);
        }
        let keep: Vec<u64> = self.journal_index.keys().copied().collect();
        self.tx_counts.retain(|h, _| keep.binary_search(h).is_ok());
    }

    pub fn tip(&self) -> u64 {
        self.journal_index.keys().next_back().copied().unwrap_or(0)
    }

    /// Lowest height still on disk (a pruning node serves a suffix).
    pub fn oldest(&self) -> Option<u64> {
        self.journal_index.keys().next().copied()
    }

    pub fn meta(&self, height: u64) -> Option<BlockMeta> {
        let &(seg, off, len) = self.journal_index.get(&height)?;
        let entry = self.read_entry(seg, off, len)?;
        Some(BlockMeta {
            height,
            timestamp: entry.timestamp,
            state_hash: self.hashes.get(&height).copied(),
            tx_count: self.tx_counts.get(&height).copied().unwrap_or(0),
        })
    }

    /// Newest-first page of block metadata ending at `before` (exclusive).
    pub fn metas(&self, before: Option<u64>, limit: usize) -> Vec<BlockMeta> {
        let end = before.unwrap_or(u64::MAX);
        self.journal_index
            .range(..end)
            .rev()
            .take(limit)
            .filter_map(|(h, _)| self.meta(*h))
            .collect()
    }

    pub fn actions(&self, height: u64) -> Option<Vec<SignedAction>> {
        let &(seg, off, len) = self.journal_index.get(&height)?;
        let entry = self.read_entry(seg, off, len)?;
        decode_payload(&entry.payload)
    }

    pub fn receipts(&self, height: u64) -> Option<Vec<Receipt>> {
        if !self.journal_index.contains_key(&height) {
            return None;
        }
        let Some(&(seg, start, end)) = self.receipts_index.get(&height) else {
            return Some(Vec::new());
        };
        let mut f = File::open(receipts_path(&self.dir, seg)).ok()?;
        f.seek(SeekFrom::Start(start)).ok()?;
        let mut buf = vec![0u8; (end - start) as usize];
        f.read_exact(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf);
        Some(
            text.lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|v| v["height"].as_u64() == Some(height))
                .filter_map(|v| serde_json::from_value::<Receipt>(v["receipt"].clone()).ok())
                .collect(),
        )
    }

    fn read_entry(&self, seg: u64, off: u64, len: u32) -> Option<JournalEntry> {
        let mut f = File::open(journal_path(&self.dir, seg)).ok()?;
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf).ok()?;
        JournalEntry::try_from_slice(&buf).ok()
    }
}

/// `{"height":N,` prefix parse without a full JSON decode.
fn height_of_line(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("{\"height\":")?;
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use borsh::BorshSerialize;

    #[derive(BorshSerialize)]
    struct Entry {
        height: u64,
        timestamp: u64,
        payload: Vec<u8>,
    }

    /// Write two blocks the way machine.rs does, reopen, and read them back.
    #[test]
    fn indexes_journal_receipts_and_hashes() {
        let dir = std::env::temp_dir().join(format!("keel-archive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut journal = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("journal.bin"))
            .unwrap();
        let mut receipts = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("receipts.jsonl"))
            .unwrap();
        let mut offsets = Vec::new();
        for h in 1..=2u64 {
            let bytes = borsh::to_vec(&Entry {
                height: h,
                timestamp: 1_000 * h,
                payload: Vec::new(),
            })
            .unwrap();
            let start = std::fs::metadata(dir.join("journal.bin")).unwrap().len();
            journal
                .write_all(&(bytes.len() as u32).to_le_bytes())
                .unwrap();
            journal.write_all(&bytes).unwrap();
            offsets.push((start + 4, bytes.len() as u32));
        }
        let r = Receipt {
            index: 0,
            height: 2,
            timestamp: 2_000,
            tx_id: [7; 32],
            signer: keel_types::Address::tagged(1),
            ok: true,
            error: None,
            events: vec![],
        };
        let line = serde_json::json!({ "height": 2, "receipt": r }).to_string();
        receipts.write_all(line.as_bytes()).unwrap();
        receipts.write_all(b"\n").unwrap();
        // Record hashes through the API (as the node would), then reopen.
        {
            let mut a = Archive::open(&dir).unwrap();
            a.record(3, 0, [9; 32], 1, 0, 0, 1, 0).unwrap();
        }
        let a = Archive::open(&dir).unwrap();
        // `tip` follows the journal; a hash alone (height 3) is not a block.
        assert_eq!(a.tip(), 2);
        assert_eq!(a.meta(1).unwrap().timestamp, 1_000);
        assert!(a.meta(3).is_none());
        assert_eq!(a.hashes.get(&3), Some(&[9; 32]));
        assert_eq!(a.receipts(2).unwrap().len(), 1);
        assert_eq!(a.receipts(2).unwrap()[0].tx_id, [7; 32]);
        assert!(a.receipts(1).unwrap().is_empty());
        assert!(a.receipts(99).is_none());
        assert_eq!(
            a.metas(None, 10)
                .iter()
                .map(|m| m.height)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(a.metas(Some(2), 1)[0].height, 1);
        assert!(a.actions(1).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn height_prefix_parses() {
        assert_eq!(height_of_line("{\"height\":42,\"receipt\":{}}\n"), Some(42));
        assert_eq!(height_of_line("nope"), None);
    }
}
