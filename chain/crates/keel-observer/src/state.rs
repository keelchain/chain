//! Persistent daemon state: what was already submitted, which outbound
//! transactions were broadcast, and per-chain scan cursors. One JSON
//! file, rewritten atomically (temp file + rename) after every change.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmittedRecord {
    /// KEEL transaction id returned by the node.
    pub tx_id: String,
    pub unix_secs: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundTx {
    pub chain: String,
    /// Transaction hash, hex (display order for Bitcoin).
    pub txid: String,
    pub broadcast_unix_secs: u64,
    /// Fee this outbound is charged with, in the chain's native units,
    /// known at broadcast time (Bitcoin batches split it evenly).
    pub fee_hint: u128,
    /// Whether this daemon broadcast it (true) or found it on chain.
    pub ours: bool,
    /// Raw signed transaction (hex) when we built it, for rebroadcast.
    #[serde(default)]
    pub raw: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StateData {
    /// Idempotency keys of submitted actions (`deposit:…`, `outbound:…`).
    #[serde(default)]
    pub submitted: BTreeMap<String, SubmittedRecord>,
    /// outbound id → broadcast transaction.
    #[serde(default)]
    pub outbound_txs: BTreeMap<u64, OutboundTx>,
    /// Scan cursors (`eth:last_block`, `tron:USDT:min_timestamp`, …).
    #[serde(default)]
    pub cursors: BTreeMap<String, u64>,
    /// Outbound ids whose batch we first saw at this unix time (for the
    /// leader timeout).
    #[serde(default)]
    pub first_seen: BTreeMap<u64, u64>,
}

pub struct StateFile {
    path: Option<PathBuf>,
    data: Mutex<StateData>,
}

impl StateFile {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let data = match std::fs::read(path) {
            Ok(bytes) if !bytes.is_empty() => serde_json::from_slice(&bytes)?,
            Ok(_) => StateData::default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StateData::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path: Some(path.to_path_buf()),
            data: Mutex::new(data),
        })
    }

    /// In-memory only (tests).
    pub fn ephemeral() -> Self {
        Self {
            path: None,
            data: Mutex::new(StateData::default()),
        }
    }

    pub fn read<R>(&self, f: impl FnOnce(&StateData) -> R) -> R {
        let d = self.data.lock().expect("state lock");
        f(&d)
    }

    /// Mutate and persist.
    pub fn update<R>(&self, f: impl FnOnce(&mut StateData) -> R) -> anyhow::Result<R> {
        let mut d = self.data.lock().expect("state lock");
        let r = f(&mut d);
        if let Some(path) = &self.path {
            let tmp = path.with_extension("json.tmp");
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(&tmp, serde_json::to_vec_pretty(&*d)?)?;
            std::fs::rename(&tmp, path)?;
        }
        Ok(r)
    }

    pub fn is_submitted(&self, key: &str) -> bool {
        self.read(|d| d.submitted.contains_key(key))
    }

    pub fn mark_submitted(&self, key: &str, tx_id: &str) -> anyhow::Result<()> {
        let unix_secs = now_unix();
        self.update(|d| {
            d.submitted.insert(
                key.to_string(),
                SubmittedRecord {
                    tx_id: tx_id.to_string(),
                    unix_secs,
                },
            );
        })
    }

    pub fn cursor(&self, key: &str) -> Option<u64> {
        self.read(|d| d.cursors.get(key).copied())
    }

    pub fn set_cursor(&self, key: &str, value: u64) -> anyhow::Result<()> {
        self.update(|d| {
            d.cursors.insert(key.to_string(), value);
        })
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("keel-observer-state-{}", std::process::id()));
        let path = dir.join("state.json");
        let s = StateFile::load(&path).unwrap();
        assert!(!s.is_submitted("deposit:BTC:aa:0"));
        s.mark_submitted("deposit:BTC:aa:0", "01").unwrap();
        s.set_cursor("eth:last_block", 42).unwrap();
        s.update(|d| {
            d.outbound_txs.insert(
                7,
                OutboundTx {
                    chain: "ETH".into(),
                    txid: "0xab".into(),
                    broadcast_unix_secs: 1,
                    fee_hint: 5,
                    ours: true,
                    raw: None,
                },
            );
        })
        .unwrap();
        let again = StateFile::load(&path).unwrap();
        assert!(again.is_submitted("deposit:BTC:aa:0"));
        assert_eq!(again.cursor("eth:last_block"), Some(42));
        assert_eq!(again.read(|d| d.outbound_txs[&7].txid.clone()), "0xab");
        let _ = std::fs::remove_dir_all(dir);
    }
}
