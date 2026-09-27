//! Vault view rebuilt from block receipts.
//!
//! `GET /v1/vaults/{chain}` and `GET /v1/vaults/outbounds` serialize the
//! whole `VaultsState`, whose maps are keyed by tuples; `serde_json`
//! refuses those, so both routes answer `null`/`[]` as soon as one vault
//! or deposit index exists (API.md, "Required upstream changes"). Until
//! the node serves a proper view, an observer configured with
//! `[vault_fallback]` folds the events of every block
//! (`GET /v1/blocks/{h}/receipts`) into this structure:
//!
//! - `DepositAddressAssigned` → deposit index → owner (per chain);
//! - `VaultRegistered` → the active epoch (the key comes from the config
//!   or the `local:` signer, which is the key that was registered);
//! - `WithdrawalQueued` → an outbound row; `OutboundConfirmed` /
//!   `OutboundFailed` close it. Batching is an end-of-block event that the
//!   receipts route does not carry, so it is inferred from
//!   `outbound_batch_interval_blocks` exactly as `vaults::end_block`
//!   batches: a queued outbound is `Batched` once the chain passed the
//!   next multiple of the interval, and that height stands in for the
//!   batch id (consistent across observers, which is all the leader
//!   election needs).
//!
//! Known gaps of the fallback: receipts are only served for the last
//! 10,000 blocks, so an observer started late or restarted misses older
//! deposit-address assignments; `fee_estimate` of outbounds is unknown
//! (0) — the Bitcoin path reads the paid fee from the wallet instead;
//! halted assets are not modelled.

use crate::rpc::{chain_from_json, OutboundRow};
use keel_actions::Chain;
use keel_types::{Address, Amount};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct EventsView {
    /// Height of the last folded block.
    pub cursor: u64,
    pub epochs: BTreeMap<Chain, u64>,
    pub owners: BTreeMap<Chain, BTreeMap<u64, Address>>,
    /// outbound id → (row, height it was queued at).
    pub outbounds: BTreeMap<u64, OutboundRow>,
}

fn address_of(v: &Value) -> Option<Address> {
    match v {
        Value::String(s) => Address::from_hex(s),
        Value::Array(a) => {
            let bytes: Option<Vec<u8>> = a
                .iter()
                .map(|x| x.as_u64().and_then(|b| u8::try_from(b).ok()))
                .collect();
            let bytes: [u8; 32] = bytes?.try_into().ok()?;
            Some(Address::from_bytes(bytes))
        }
        _ => None,
    }
}

fn amount_of(v: &Value) -> Option<Amount> {
    match v {
        Value::Number(n) => n.as_u128(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Chain of a vault asset from its `CHAIN.SYMBOL` prefix.
pub fn chain_of_asset(asset: &str) -> Option<Chain> {
    chain_from_json(asset.split_once('.')?.0)
}

fn native_asset(chain: Chain) -> &'static str {
    match chain {
        Chain::Bitcoin => "BTC.BTC",
        Chain::Ethereum => "ETH.ETH",
        Chain::Tron => "TRON.TRX",
    }
}

impl EventsView {
    /// Fold the receipts of block `height` (the `receipts` array of
    /// `GET /v1/blocks/{height}/receipts`).
    pub fn fold_block(&mut self, height: u64, receipts: &[Value]) {
        for r in receipts {
            if r["ok"].as_bool() != Some(true) {
                continue;
            }
            for e in r["events"].as_array().cloned().unwrap_or_default() {
                self.fold_event(height, &e);
            }
        }
        self.cursor = self.cursor.max(height);
    }

    fn fold_event(&mut self, height: u64, e: &Value) {
        let Some((name, body)) = e.as_object().and_then(|o| o.iter().next()) else {
            return;
        };
        match name.as_str() {
            "DepositAddressAssigned" => {
                let (Some(chain), Some(index), Some(owner)) = (
                    body["chain"].as_str().and_then(chain_from_json),
                    body["index"].as_u64(),
                    address_of(&body["owner"]),
                ) else {
                    return;
                };
                self.owners.entry(chain).or_default().insert(index, owner);
            }
            "VaultRegistered" => {
                let (Some(chain), Some(epoch)) = (
                    body["chain"].as_str().and_then(chain_from_json),
                    body["epoch"].as_u64(),
                ) else {
                    return;
                };
                let e = self.epochs.entry(chain).or_default();
                *e = (*e).max(epoch);
            }
            "WithdrawalQueued" => {
                let (Some(id), Some(owner), Some(asset), Some(amount)) = (
                    body["outbound_id"].as_u64(),
                    address_of(&body["owner"]),
                    body["asset"].as_str(),
                    amount_of(&body["amount"]),
                ) else {
                    return;
                };
                let Some(chain) = chain_of_asset(asset) else {
                    return;
                };
                self.outbounds.insert(
                    id,
                    OutboundRow {
                        id,
                        owner,
                        asset: asset.to_string(),
                        chain,
                        to: body["to"].as_str().unwrap_or_default().to_string(),
                        amount,
                        fee_asset: native_asset(chain).to_string(),
                        fee_estimate: 0,
                        status: "Queued".into(),
                        batch_id: None,
                        created_height: height,
                        tx_hash: None,
                        custodian: None,
                    },
                );
            }
            "OutboundConfirmed" => {
                if let Some(o) = body["outbound_id"]
                    .as_u64()
                    .and_then(|id| self.outbounds.get_mut(&id))
                {
                    o.status = "Confirmed".into();
                    o.tx_hash = body["tx_hash"].as_str().map(str::to_string);
                }
            }
            "OutboundFailed" => {
                if let Some(o) = body["outbound_id"]
                    .as_u64()
                    .and_then(|id| self.outbounds.get_mut(&id))
                {
                    o.status = "Failed".into();
                }
            }
            _ => {}
        }
    }

    /// Height at which an outbound queued at `created` is batched.
    pub fn batch_height(created: u64, interval: u64) -> u64 {
        let interval = interval.max(1);
        created.div_ceil(interval) * interval
    }

    /// Outbound rows with the inferred `Batched` status for the ones whose
    /// batch height the chain has reached (`tip` is the folded height).
    pub fn outbounds(&self, interval: u64, tip: u64) -> Vec<OutboundRow> {
        self.outbounds
            .values()
            .map(|o| {
                let mut o = o.clone();
                if o.status == "Queued" {
                    let h = Self::batch_height(o.created_height, interval);
                    if tip >= h {
                        o.status = "Batched".into();
                        o.batch_id = Some(h);
                    }
                }
                o
            })
            .collect()
    }

    pub fn next_deposit_index(&self, chain: Chain) -> u64 {
        self.owners
            .get(&chain)
            .and_then(|m| m.keys().next_back())
            .map_or(1, |i| i + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn folds_receipts_and_infers_batches() {
        let alice = Address::tagged(1);
        let mut v = EventsView::default();
        let assign = json!({ "ok": true, "events": [
            { "DepositAddressAssigned": { "owner": serde_json::to_value(alice).unwrap(), "chain": "BTC", "index": 1 } },
            { "VaultRegistered": { "chain": "BTC", "epoch": 2 } },
            { "VaultRegistered": { "chain": "BTC", "epoch": 1 } },
        ]});
        let failed = json!({ "ok": false, "events": [ { "DepositAddressAssigned": { "owner": serde_json::to_value(alice).unwrap(), "chain": "BTC", "index": 9 } } ] });
        v.fold_block(5, &[assign, failed]);
        assert_eq!(v.owners[&Chain::Bitcoin][&1], alice);
        assert_eq!(v.next_deposit_index(Chain::Bitcoin), 2);
        assert_eq!(v.next_deposit_index(Chain::Tron), 1);
        assert_eq!(v.epochs[&Chain::Bitcoin], 2);

        let queued = json!({ "ok": true, "events": [
            { "WithdrawalQueued": { "outbound_id": 0, "owner": alice.to_hex(), "asset": "BTC.BTC", "amount": "150000", "to": "bcrt1qxyz" } },
            { "WithdrawalQueued": { "outbound_id": 1, "owner": alice.to_hex(), "asset": "TRON.USDT", "amount": 7, "to": "Tabc" } },
        ]});
        v.fold_block(23, &[queued]);
        assert_eq!(v.cursor, 23);
        let rows = v.outbounds(20, 23);
        assert!(rows.iter().all(|r| r.status == "Queued"));
        let rows = v.outbounds(20, 40);
        assert_eq!(rows[0].status, "Batched");
        assert_eq!(rows[0].batch_id, Some(40));
        assert_eq!(rows[0].chain, Chain::Bitcoin);
        assert_eq!(rows[0].fee_asset, "BTC.BTC");
        assert_eq!(rows[0].amount, 150_000);
        assert_eq!(rows[1].chain, Chain::Tron);
        assert_eq!(EventsView::batch_height(40, 20), 40);
        assert_eq!(EventsView::batch_height(0, 20), 0);

        v.fold_block(41, &[json!({ "ok": true, "events": [ { "OutboundConfirmed": { "outbound_id": 0, "tx_hash": "ab" } } ] })]);
        let rows = v.outbounds(20, 60);
        assert_eq!(rows[0].status, "Confirmed");
        assert_eq!(rows[0].tx_hash.as_deref(), Some("ab"));
        assert_eq!(rows[1].status, "Batched");
    }
}
