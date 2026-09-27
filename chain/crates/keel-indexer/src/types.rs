//! Wire models of what the node serves, parsed leniently from JSON so a new
//! event variant or field never stops indexing (unknown events are stored
//! raw and simply not materialized).

use serde_json::Value;

pub type Amount = u128;

/// `u128` amounts arrive as JSON numbers from the VM's serde and as decimal
/// strings from the node's hand-built views. Accept both.
pub fn amount_of(v: &Value) -> Option<Amount> {
    match v {
        Value::Number(n) => n
            .as_u128()
            .or_else(|| n.as_u64().map(u128::from))
            .or_else(|| n.as_i64().and_then(|i| u128::try_from(i).ok())),
        Value::String(s) => s.trim().parse::<u128>().ok(),
        _ => None,
    }
}

pub fn field_amount(v: &Value, key: &str) -> Option<Amount> {
    v.get(key).and_then(amount_of)
}

pub fn field_u64(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(|x| {
        x.as_u64()
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
    })
}

pub fn field_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Addresses are 64-hex strings; older encoders emit 32-byte arrays.
pub fn address_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            let s = s.trim().trim_start_matches("0x");
            (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| s.to_ascii_lowercase())
        }
        Value::Array(a) if a.len() == 32 => {
            let bytes: Option<Vec<u8>> = a
                .iter()
                .map(|x| x.as_u64().and_then(|n| u8::try_from(n).ok()))
                .collect();
            bytes.map(hex::encode)
        }
        _ => None,
    }
}

pub fn field_addr(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(address_of)
}

pub fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// An event is externally tagged: `{"OrderFilled": {...}}` or a bare
/// `"SomeUnitVariant"`. Returns `(type, fields)`.
pub fn split_event(v: &Value) -> (String, Value) {
    match v {
        Value::Object(m) if m.len() == 1 => {
            let (k, inner) = m
                .iter()
                .next()
                .map(|(k, v)| (k.clone(), v.clone()))
                .unwrap_or_default();
            (k, inner)
        }
        Value::String(s) => (s.clone(), Value::Object(Default::default())),
        Value::Object(m) => {
            // Already flattened `{type, ...}` (our own API shape).
            let t = m
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_string();
            let mut rest = m.clone();
            rest.remove("type");
            (t, Value::Object(rest))
        }
        _ => ("Unknown".into(), v.clone()),
    }
}

/// Keys whose 64-hex values are hashes, not addresses.
const HASH_KEYS: &[&str] = &[
    "tx_hash",
    "hash",
    "evidence_hash",
    "instructions_hash",
    "proof_hash",
    "digest",
    "state_hash",
    "tx_id",
    "committee_root",
    "next_committee_root",
    "genesis_validators_root",
];

/// Every address a JSON value mentions (deduplicated, sorted).
pub fn addresses_in(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(v, None, &mut out);
    out.sort();
    out.dedup();
    out
}

fn walk(v: &Value, key: Option<&str>, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                walk(x, Some(k.as_str()), out);
            }
        }
        Value::Array(a) => {
            if let Some(addr) = address_of(v) {
                if !key.is_some_and(|k| HASH_KEYS.contains(&k)) {
                    out.push(addr);
                }
                return;
            }
            for x in a {
                walk(x, key, out);
            }
        }
        Value::String(_) => {
            if key.is_some_and(|k| HASH_KEYS.contains(&k)) {
                return;
            }
            if let Some(addr) = address_of(v) {
                out.push(addr);
            }
        }
        _ => {}
    }
}

/// One receipt as the node serves it (`receipt_json` in keel-rpc).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptRow {
    pub index: u32,
    pub tx_id: String,
    pub signer: String,
    pub ok: bool,
    pub error: Option<(String, String)>,
    pub events: Vec<Value>,
    pub timestamp: Option<u64>,
}

pub fn parse_receipt(v: &Value) -> Option<ReceiptRow> {
    let tx_id = field_str(v, "tx_id")?.to_ascii_lowercase();
    let error = v.get("error").and_then(|e| {
        if e.is_null() {
            None
        } else {
            Some((
                field_str(e, "code").unwrap_or_else(|| "ERROR".into()),
                field_str(e, "message").unwrap_or_default(),
            ))
        }
    });
    Some(ReceiptRow {
        index: field_u64(v, "index")? as u32,
        tx_id,
        signer: field_addr(v, "signer").unwrap_or_default(),
        ok: v
            .get("ok")
            .and_then(Value::as_bool)
            .unwrap_or(error.is_none()),
        error,
        events: v
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        timestamp: field_u64(v, "timestamp"),
    })
}

/// One decoded action from `GET /v1/blocks/{h}/actions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionRow {
    pub index: u32,
    pub tx_id: String,
    pub signer: String,
    pub nonce: Option<u64>,
    /// serde JSON of `keel_actions::Action` (externally tagged).
    pub action: Value,
}

pub fn parse_actions(v: &Value) -> Vec<ActionRow> {
    v.get("actions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(ActionRow {
                        index: field_u64(x, "index")? as u32,
                        tx_id: field_str(x, "tx_id")
                            .unwrap_or_default()
                            .to_ascii_lowercase(),
                        signer: field_addr(x, "signer").unwrap_or_default(),
                        nonce: field_u64(x, "nonce"),
                        action: x.get("action").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Everything known about one block, from whichever route served it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockData {
    pub height: u64,
    /// Block time (ms) when the node served one.
    pub timestamp: Option<u64>,
    pub state_hash: Option<String>,
    pub proposer: Option<String>,
    pub receipts: Vec<ReceiptRow>,
    /// End-of-block events (no receipt).
    pub events: Vec<Value>,
    /// `None` when the node has no actions route (see README, node API gaps).
    pub actions: Option<Vec<ActionRow>>,
}

impl BlockData {
    /// Best block time: the header's, else the first receipt's.
    pub fn best_timestamp(&self) -> Option<u64> {
        self.timestamp
            .or_else(|| self.receipts.iter().find_map(|r| r.timestamp))
    }
}

/// A `/v1/ws` message (`BlockUpdate` in keel-rpc) or a `/v1/blocks/{h}` body.
pub fn parse_block_update(v: &Value) -> Option<BlockData> {
    // The node's socket also carries heartbeats, gaps and channel events;
    // only `block` frames (or untyped frames from an older node) are blocks.
    if let Some(t) = field_str(v, "type") {
        if t != "block" {
            return None;
        }
    }
    let height = field_u64(v, "height")?;
    let receipts = v
        .get("receipts")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(parse_receipt).collect())
        .unwrap_or_default();
    Some(BlockData {
        height,
        timestamp: field_u64(v, "timestamp"),
        state_hash: field_str(v, "state_hash").map(|s| s.to_ascii_lowercase()),
        proposer: field_str(v, "proposer"),
        receipts,
        events: v
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        actions: v.get("actions").map(|_| parse_actions(v)),
    })
}

#[derive(Clone, Debug, Default)]
pub struct NodeStatus {
    pub chain_id: u64,
    pub height: u64,
    pub timestamp: u64,
    pub state_hash: String,
    pub validators: Vec<String>,
}

pub fn parse_status(v: &Value) -> Option<NodeStatus> {
    Some(NodeStatus {
        chain_id: field_u64(v, "chain_id").unwrap_or(0),
        height: field_u64(v, "height")?,
        timestamp: field_u64(v, "timestamp").unwrap_or(0),
        state_hash: field_str(v, "state_hash")
            .unwrap_or_default()
            .to_ascii_lowercase(),
        validators: v
            .get("validators")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Genesis asset table (`keel_vm::Genesis::devnet`); the node has no assets
/// route, so this seeds decimals until a market config or RegisterAsset
/// proposal says otherwise.
pub const GENESIS_ASSETS: &[(&str, u32, &str)] = &[
    ("KEEL", 6, "native"),
    ("KUSD", 6, "stable"),
    ("BTC.BTC", 8, "vault"),
    ("ETH.ETH", 18, "vault"),
    ("ETH.USDT", 6, "vault"),
    ("TRON.TRX", 6, "vault"),
    ("TRON.USDT", 6, "vault"),
];

pub fn asset_chain(asset: &str) -> Option<&str> {
    asset.split_once('.').map(|(c, _)| c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn amounts_accept_numbers_and_strings() {
        assert_eq!(amount_of(&json!(5)), Some(5));
        assert_eq!(
            amount_of(&json!("340282366920938463463374607431768211455")),
            Some(u128::MAX)
        );
        assert_eq!(
            amount_of(&json!(18446744073709551615u64)),
            Some(u128::from(u64::MAX))
        );
        assert_eq!(
            amount_of(
                &serde_json::from_str::<Value>("340282366920938463463374607431768211455")
                    .unwrap_or(Value::Null)
            )
            .or(Some(0)),
            Some(0),
            "beyond u64 the JSON number does not parse into Value (strings do)"
        );
        assert_eq!(amount_of(&json!(null)), None);
    }

    #[test]
    fn addresses_from_hex_and_bytes_but_not_hashes() {
        let hex = "ab".repeat(32);
        let ev = json!({"Transferred": {"from": hex, "to": vec![7u8; 32], "asset": "KEEL", "amount": 1, "tx_hash": "cd".repeat(32)}});
        let addrs = addresses_in(&ev);
        assert_eq!(addrs, vec!["07".repeat(32), "ab".repeat(32)]);
        let (t, inner) = split_event(&ev);
        assert_eq!(t, "Transferred");
        assert_eq!(field_amount(&inner, "amount"), Some(1));
    }

    #[test]
    fn receipt_and_block_update_parse() {
        let v = json!({
            "height": 10, "timestamp": 1000, "state_hash": "AB",
            "receipts": [{"index": 0, "tx_id": "FF".repeat(32), "signer": "11".repeat(32), "ok": false,
                          "error": {"code": "INVALID", "message": "nope"}, "events": [], "timestamp": 999}],
            "events": [{"EpochAdvanced": {"epoch": 1, "validators": 4}}]
        });
        let b = parse_block_update(&v).unwrap();
        assert_eq!(b.height, 10);
        assert_eq!(b.state_hash.as_deref(), Some("ab"));
        assert_eq!(b.receipts[0].error.as_ref().unwrap().0, "INVALID");
        assert!(!b.receipts[0].ok);
        assert_eq!(b.events.len(), 1);
        assert!(b.actions.is_none());
    }
}
