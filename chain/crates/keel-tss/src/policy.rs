//! The signing policy of `keel-tss serve --policy-rpc <node>`: a request
//! is signed only when its context binds the digest to a transaction that
//! pays open outbounds of a batch the chain finalized
//! (`keel_chains::policy::verify`). The outbound rows come from the node
//! this signer trusts; the check runs on the coordinator before it
//! announces a session and on every joiner before it takes part.
//!
//! One policy serves both key families: the CGGMP21 share answers for the
//! secp256k1 chains, the FROST share for Solana; a request for a family
//! this signer has no share of is refused.

use keel_chains::policy::{verify_with_pool, OutboundRef, PoolRef, SignContext};
use keel_tss::{
    ecdsa::{self, KeyShare},
    eddsa::{self, EddsaShare},
};
use std::time::Duration;

pub struct Policy {
    rpc: String,
    /// (compressed vault key, chain code) of the CGGMP21 share.
    ecdsa: Option<([u8; 33], [u8; 32])>,
    /// (vault key, chain code) of the FROST share.
    eddsa: Option<([u8; 32], [u8; 32])>,
}

impl Policy {
    pub fn new(
        rpc: &str,
        ecdsa_share: Option<&KeyShare>,
        eddsa_share: Option<&EddsaShare>,
    ) -> anyhow::Result<Self> {
        let ecdsa = match ecdsa_share {
            Some(share) => {
                let chain_code = ecdsa::chain_code(share).ok_or_else(|| {
                    anyhow::anyhow!("the share has no chain code; policy needs an HD share")
                })?;
                Some((ecdsa::vault_public_key(share), chain_code))
            }
            None => None,
        };
        let eddsa = eddsa_share.map(|s| (eddsa::vault_public_key(s), eddsa::chain_code(s)));
        if ecdsa.is_none() && eddsa.is_none() {
            anyhow::bail!("policy needs at least one share");
        }
        Ok(Self {
            rpc: rpc.trim_end_matches('/').to_string(),
            ecdsa,
            eddsa,
        })
    }

    /// The vault key and chain code answering for the context's chain.
    fn keys_for(&self, ctx: &SignContext) -> Result<(Vec<u8>, [u8; 32]), String> {
        if ctx.chain().is_ed25519() {
            self.eddsa
                .map(|(pk, cc)| (pk.to_vec(), cc))
                .ok_or_else(|| "no eddsa (FROST) share for a Solana request".to_string())
        } else {
            self.ecdsa
                .map(|(pk, cc)| (pk.to_vec(), cc))
                .ok_or_else(|| "no ecdsa (CGGMP21) share for this request".to_string())
        }
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
        let (vault_pubkey, chain_code) = self.keys_for(ctx)?;
        let rows = if ctx.batch_id().is_some() {
            self.outbounds()?
        } else {
            Vec::new()
        };
        let pool = match (ctx.needs_pool(), ctx.custodian()) {
            (true, Some(c)) => Some(self.pool(c, ctx.chain(), &vault_pubkey)?),
            _ => None,
        };
        verify_with_pool(
            ctx,
            digest,
            path,
            &vault_pubkey,
            &chain_code,
            &rows,
            pool.as_ref(),
        )
    }

    /// The custody vault's active generation (`GET /v1/custody/{addr}`):
    /// the only place a sweep may send. It must be a different key from
    /// the one this signer holds, or the "sweep" would go nowhere.
    fn pool(
        &self,
        custodian: &str,
        chain: keel_actions::Chain,
        own_key: &[u8],
    ) -> Result<PoolRef, String> {
        let body: serde_json::Value = ureq::get(format!("{}/v1/custody/{custodian}", self.rpc))
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .call()
            .map_err(|e| format!("policy rpc: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| format!("policy rpc body: {e}"))?;
        let active = body
            .get("vaults")
            .and_then(|v| v.as_array())
            .and_then(|list| {
                list.iter().find(|e| {
                    e.get("chain").and_then(|c| c.as_str()) == Some(chain.as_str())
                        && e.get("status").and_then(|s| s.as_str()) == Some("Active")
                })
            })
            .ok_or_else(|| "policy rpc: no active generation for that custodian".to_string())?;
        let hexbytes = |v: &serde_json::Value| -> Option<Vec<u8>> {
            v.as_str().and_then(|s| hex::decode(s).ok())
        };
        let pk = hexbytes(&active["vault"]["public_key"])
            .filter(|b| b.len() == chain.key_length())
            .ok_or_else(|| "policy rpc: bad active public key".to_string())?;
        let cc: [u8; 32] = hexbytes(&active["vault"]["chain_code"])
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| "policy rpc: bad active chain code".to_string())?;
        if pk == own_key {
            return Err("the active generation is this very key; nothing to sweep to".into());
        }
        Ok(PoolRef {
            active_pubkey: pk,
            active_chain_code: cc,
        })
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
                Some(s) => keel_actions::Chain::parse(s)
                    .ok_or_else(|| format!("policy rpc: unknown chain {s:?}"))?,
                None => return Err("policy rpc: outbound without a chain".into()),
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
                asset: o
                    .get("asset")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
        Ok(rows)
    }
}
