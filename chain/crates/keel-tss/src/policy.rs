//! The signing policy of `keel-tss serve --policy-rpc <node>`: a request
//! is signed only when its context binds the digest to a transaction that
//! pays open outbounds of a batch the chain finalized
//! (`keel_chains::policy::verify`). The outbound rows come from the node
//! this signer trusts; the check runs on the coordinator before it
//! announces a session and on every joiner before it takes part.

use keel_chains::policy::{verify, OutboundRef, SignContext};
use keel_tss::ecdsa::{self, KeyShare};
use std::time::Duration;

pub struct Policy {
    rpc: String,
    vault_pubkey: [u8; 33],
    chain_code: [u8; 32],
}

impl Policy {
    pub fn new(rpc: &str, share: &KeyShare) -> anyhow::Result<Self> {
        let chain_code = ecdsa::chain_code(share).ok_or_else(|| {
            anyhow::anyhow!("the share has no chain code; policy needs an HD share")
        })?;
        Ok(Self {
            rpc: rpc.trim_end_matches('/').to_string(),
            vault_pubkey: ecdsa::vault_public_key(share),
            chain_code,
        })
    }

    /// Refuses a request without context, or whose context does not hold.
    pub fn check(
        &self,
        digest: &[u8; 32],
        path: &[u32],
        ctx: Option<&SignContext>,
    ) -> Result<(), String> {
        let ctx = ctx.ok_or_else(|| {
            "signing request carries no context and a policy is configured".to_string()
        })?;
        let rows = self.outbounds()?;
        verify(
            ctx,
            digest,
            path,
            &self.vault_pubkey,
            &self.chain_code,
            &rows,
        )
    }

    fn outbounds(&self) -> Result<Vec<OutboundRef>, String> {
        let body: serde_json::Value = ureq::get(format!("{}/v1/vaults/outbounds", self.rpc))
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .call()
            .map_err(|e| format!("policy rpc: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| format!("policy rpc body: {e}"))?;
        let list = body
            .get("outbounds")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "policy rpc: no outbounds list".to_string())?;
        let mut rows = Vec::with_capacity(list.len());
        for o in list {
            let chain = match o.get("chain").and_then(|c| c.as_str()) {
                Some("BTC") => keel_actions::Chain::Bitcoin,
                Some("ETH") => keel_actions::Chain::Ethereum,
                Some("TRON") => keel_actions::Chain::Tron,
                other => return Err(format!("policy rpc: unknown chain {other:?}")),
            };
            let amount = o
                .get("amount")
                .and_then(|a| a.as_str())
                .and_then(|a| a.parse::<u128>().ok())
                .ok_or_else(|| "policy rpc: bad amount".to_string())?;
            rows.push(OutboundRef {
                id: o.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                chain,
                to: o
                    .get("to")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                amount,
                status: o
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                batch_id: o.get("batch_id").and_then(|v| v.as_u64()),
            });
        }
        Ok(rows)
    }
}
