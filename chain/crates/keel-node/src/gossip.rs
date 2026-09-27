//! Action gossip: every admitted action is broadcast once to all peers;
//! received actions are admitted into the local mempool and re-broadcast
//! the first time they are seen (the tx-id set in the mempool dedupes).
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use crate::machine::Shared;
use commonware_p2p::{Receiver, Recipients, Sender};
use keel_actions::SignedAction;
use keel_consensus::types::PublicKey;
use keel_rpc::NodeApi;
use keel_vm::VmError;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Node-side implementation of the RPC's [`NodeApi`].
pub struct Node {
    pub shared: Shared,
    pub outbound: mpsc::UnboundedSender<SignedAction>,
    pub validators: Vec<String>,
    /// External network the vault addresses are encoded for.
    pub external_network: keel_chains::Network,
}

impl Node {
    /// Admit into the mempool; returns whether it was newly inserted.
    pub fn admit(&self, action: &SignedAction) -> Result<bool, VmError> {
        let state = self.shared.state.lock().expect("state lock");
        let mut mempool = self.shared.mempool.lock().expect("mempool lock");
        mempool.insert(&state, action.clone())
    }
}

impl NodeApi for Node {
    fn state(&self) -> std::sync::Arc<std::sync::Mutex<keel_vm::State>> {
        self.shared.state.clone()
    }

    fn submit(&self, action: SignedAction) -> Result<[u8; 32], VmError> {
        let id = action.id();
        if self.admit(&action)? {
            let _ = self.outbound.send(action);
        }
        Ok(id)
    }

    fn mempool_len(&self) -> usize {
        self.shared.mempool.lock().expect("mempool lock").len()
    }

    fn receipt(&self, tx_id: &[u8; 32]) -> Option<keel_vm::receipt::Receipt> {
        self.shared
            .receipts
            .lock()
            .expect("receipts lock")
            .get(tx_id)
    }

    fn receipts_at(&self, height: u64) -> Option<Vec<keel_vm::receipt::Receipt>> {
        self.shared
            .receipts
            .lock()
            .expect("receipts lock")
            .at(height)
    }

    fn external_network(&self) -> keel_chains::Network {
        self.external_network
    }

    fn last_credit(&self, chain: &str) -> Option<(u64, u64)> {
        self.shared
            .last_credit
            .lock()
            .expect("last credit lock")
            .get(chain)
            .copied()
    }

    fn version(&self) -> String {
        crate::machine::NODE_VERSION.to_string()
    }

    fn oldest_block(&self) -> Option<u64> {
        self.shared.archive.lock().expect("archive lock").oldest()
    }

    fn sync_meta(&self) -> Option<keel_rpc::SyncMeta> {
        let (height, _, meta) = crate::machine::Storage::newest_snapshot_file(&self.shared.dir)?;
        let boundaries = self
            .shared
            .boundaries
            .lock()
            .expect("boundaries lock")
            .values()
            .filter(|b| b.height <= height)
            .map(|b| keel_rpc::SyncBoundary {
                epoch: b.epoch,
                height: b.height,
                digest: hex::encode(b.digest),
            })
            .collect();
        Some(keel_rpc::SyncMeta {
            height,
            state_hash: meta.state_hash,
            last_hash: meta.last_hash,
            schema: meta.schema,
            boundaries,
        })
    }

    fn sync_snapshot(&self) -> Option<Vec<u8>> {
        let (_, path, _) = crate::machine::Storage::newest_snapshot_file(&self.shared.dir)?;
        std::fs::read(path).ok()
    }

    fn block_meta(&self, height: u64) -> Option<keel_rpc::BlockMeta> {
        self.shared
            .archive
            .lock()
            .expect("archive lock")
            .meta(height)
            .map(meta_json)
    }

    fn block_metas(&self, before: Option<u64>, limit: usize) -> Vec<keel_rpc::BlockMeta> {
        self.shared
            .archive
            .lock()
            .expect("archive lock")
            .metas(before, limit)
            .into_iter()
            .map(meta_json)
            .collect()
    }

    fn block_actions(&self, height: u64) -> Option<Vec<keel_actions::SignedAction>> {
        self.shared
            .archive
            .lock()
            .expect("archive lock")
            .actions(height)
    }

    fn receipts_archived(&self, height: u64) -> Option<Vec<keel_vm::receipt::Receipt>> {
        self.shared
            .archive
            .lock()
            .expect("archive lock")
            .receipts(height)
    }

    fn events_at(&self, height: u64) -> Option<Vec<keel_vm::Event>> {
        self.shared
            .receipts
            .lock()
            .expect("receipts lock")
            .events_at(height)
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<keel_rpc::BlockUpdate> {
        self.shared.updates.subscribe()
    }

    fn validators(&self) -> Vec<String> {
        self.validators.clone()
    }
}

/// Drain locally admitted actions to all peers.
pub async fn send_loop(
    mut sender: impl Sender<PublicKey = PublicKey>,
    mut rx: mpsc::UnboundedReceiver<SignedAction>,
) {
    while let Some(action) = rx.recv().await {
        let bytes = borsh::to_vec(&action).unwrap_or_default();
        let _ = sender.send(Recipients::All, bytes, false);
    }
}

/// Admit peers' actions; re-broadcast the ones we had not seen.
pub async fn recv_loop(
    node: std::sync::Arc<Node>,
    mut receiver: impl Receiver<PublicKey = PublicKey>,
) {
    loop {
        let (peer, msg) = match receiver.recv().await {
            Ok(m) => m,
            Err(e) => {
                warn!(?e, "action channel closed");
                return;
            }
        };
        let Ok(action) = borsh::from_slice::<SignedAction>(msg.as_ref()) else {
            debug!(?peer, "undecodable action");
            continue;
        };
        match node.admit(&action) {
            Ok(true) => {
                let _ = node.outbound.send(action);
            }
            Ok(false) => {}
            Err(e) => debug!(?peer, %e, "action refused"),
        }
    }
}

fn meta_json(m: crate::archive::BlockMeta) -> keel_rpc::BlockMeta {
    keel_rpc::BlockMeta {
        height: m.height,
        timestamp: m.timestamp,
        state_hash: m.state_hash.map(hex::encode),
        tx_count: m.tx_count,
    }
}
