//! State sync: bootstrap an empty node from a peer's newest snapshot.
//!
//! `bootstrap` fetches `/v1/sync/meta` and `/v1/sync/snapshot` from one RPC,
//! checks that the bytes decode to a state whose full hash and tip hash are
//! the ones the peer claims, optionally cross-checks the tip hash against a
//! second peer's block record, and installs the snapshot together with the
//! epoch boundaries into the VM storage directory. The node then opens its
//! storage as after a restart and lets marshal fetch the blocks above the
//! snapshot from its peers.

use crate::machine::{Boundary, SnapshotMeta, Storage};
use anyhow::{bail, ensure, Context as _};
use keel_rpc::SyncMeta;
use keel_vm::{migrate, State};
use std::path::Path;

/// Installs a snapshot from `from` unless `vm_dir` already holds one.
pub fn bootstrap(vm_dir: &Path, from: &str, verify: Option<&str>) -> anyhow::Result<()> {
    if let Some((h, _, _)) = Storage::newest_snapshot_file(vm_dir) {
        // Logging starts with the runtime, after this step: print directly.
        eprintln!("state sync: snapshot at height {h} present; skipping");
        return Ok(());
    }
    let from = from.trim_end_matches('/');
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let client = reqwest::Client::new();
        let meta: SyncMeta = client
            .get(format!("{from}/v1/sync/meta"))
            .send()
            .await
            .context("sync meta request")?
            .error_for_status()
            .context("sync meta")?
            .json()
            .await
            .context("sync meta body")?;
        eprintln!(
            "state sync: fetching snapshot at height {} (schema {}, {} epoch boundaries) from {from}",
            meta.height,
            meta.schema,
            meta.boundaries.len()
        );
        let bytes = client
            .get(format!("{from}/v1/sync/snapshot"))
            .send()
            .await
            .context("snapshot request")?
            .error_for_status()
            .context("snapshot")?
            .bytes()
            .await
            .context("snapshot body")?;
        let (schema, _) = migrate::split(&bytes);
        ensure!(
            schema <= migrate::SCHEMA,
            "peer snapshot schema {schema} is newer than this binary ({})",
            migrate::SCHEMA
        );
        let state = State::restore(&bytes).context("snapshot does not decode")?;
        ensure!(
            state.height == meta.height,
            "snapshot height {} differs from meta {}",
            state.height,
            meta.height
        );
        let state_hash = hex::encode(state.compute_hash());
        ensure!(
            state_hash == meta.state_hash,
            "snapshot state hash {state_hash} differs from meta {}",
            meta.state_hash
        );
        let last_hash = hex::encode(state.last_hash);
        ensure!(
            last_hash == meta.last_hash,
            "snapshot tip hash {last_hash} differs from meta {}",
            meta.last_hash
        );
        if let Some(v) = verify {
            let v = v.trim_end_matches('/');
            let block: serde_json::Value = client
                .get(format!("{v}/v1/blocks/{}", meta.height))
                .send()
                .await
                .context("verify peer block request")?
                .error_for_status()
                .context("verify peer block")?
                .json()
                .await
                .context("verify peer block body")?;
            match block.get("state_hash").and_then(|h| h.as_str()) {
                Some(h) if h == last_hash => eprintln!("state sync: {v} confirms the tip hash"),
                Some(h) => bail!(
                    "verify peer reports tip hash {h} at {}, snapshot has {last_hash}",
                    meta.height
                ),
                None => bail!("verify peer has no state hash for height {}", meta.height),
            }
        }
        let mut boundaries = Vec::with_capacity(meta.boundaries.len());
        for b in &meta.boundaries {
            let raw = hex::decode(&b.digest).context("boundary digest hex")?;
            ensure!(raw.len() == 32, "boundary digest must be 32 bytes");
            let mut digest = [0u8; 32];
            digest.copy_from_slice(&raw);
            boundaries.push(Boundary {
                epoch: b.epoch,
                height: b.height,
                digest,
            });
        }
        let installed = SnapshotMeta {
            height: meta.height,
            state_hash,
            last_hash,
            schema: migrate::SCHEMA,
        };
        Storage::install_snapshot(vm_dir, meta.height, &bytes, &installed, &boundaries)?;
        eprintln!(
            "state sync: installed snapshot at height {} ({} bytes, {} epoch boundaries) from {from}",
            meta.height,
            bytes.len(),
            boundaries.len()
        );
        Ok(())
    })
}
