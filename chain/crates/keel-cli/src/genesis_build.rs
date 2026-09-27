//! `keel genesis-build`: a real-network genesis from public keys only.
//!
//! The devnet genesis hands every role to the same seeds; a network with
//! several hosts and several parties needs the roles split: validators
//! (consensus key + account + bond), observers (the vault signers, with a
//! threshold), arbitrators, attesters, the parameter admin, and the
//! accounts funded at genesis. Nothing here needs a secret: each host
//! prints its keys with `keel-node --print-identity` and `keel keygen`,
//! and the workflow assembles them.
//!
//! ```text
//! keel genesis-build --chain-id 3 \
//!   --validator <consensus hex>:<address hex>:<bond> [--validator ...] \
//!   --observer <address> [--observer ...] --observer-threshold 2 \
//!   --arbitrator <address> --attester <address> --param-admin <address> \
//!   --fund <address>:KEEL:1000000000000 --fund <address>:KUSD:1000000000000 \
//!   --params-json infra/testnet/genesis-params.json --out genesis.json
//! ```

use anyhow::{anyhow, bail, Context as _};
use keel_types::{Address, Amount, Asset};
use keel_vm::{
    genesis::{Genesis, GenesisAccount, GenesisValidator},
    Params,
};
use std::path::PathBuf;

#[derive(clap::Args, Debug, Clone)]
pub struct Args {
    /// `<consensus key hex>:<account address hex>:<bond>`; repeat per validator.
    #[arg(long = "validator", required = true)]
    pub validators: Vec<String>,
    /// Observer (vault signer) account address; repeat per observer.
    #[arg(long = "observer")]
    pub observers: Vec<String>,
    /// Signatures needed to move the vault (defaults to 2/3 of the observers).
    #[arg(long)]
    pub observer_threshold: Option<u32>,
    #[arg(long = "arbitrator")]
    pub arbitrators: Vec<String>,
    #[arg(long = "attester")]
    pub attesters: Vec<String>,
    /// Account that may change parameters directly until governance revokes it.
    #[arg(long)]
    pub param_admin: Option<String>,
    /// House-quote operator for system-owned pairs.
    #[arg(long)]
    pub house_operator: Option<String>,
    /// `<address hex>:<ASSET>:<amount in smallest units>`; repeat per credit.
    #[arg(long = "fund")]
    pub funds: Vec<String>,
    /// JSON object of `Params` overrides; `budget` merges field by field.
    #[arg(long)]
    pub params_json: Option<PathBuf>,
    /// Write the genesis here (stdout when omitted).
    #[arg(long)]
    pub out: Option<PathBuf>,
}

fn address(s: &str, what: &str) -> anyhow::Result<Address> {
    Address::from_hex(s.trim()).ok_or_else(|| anyhow!("{what}: {s:?} is not a 64-hex address"))
}

fn addresses(list: &[String], what: &str) -> anyhow::Result<Vec<Address>> {
    list.iter().map(|s| address(s, what)).collect()
}

fn validator(s: &str) -> anyhow::Result<GenesisValidator> {
    let mut parts = s.split(':');
    let (Some(key), Some(addr), Some(bond), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("--validator must be <consensus key>:<address>:<bond>, got {s:?}");
    };
    let key = hex::decode(key.trim()).context("validator consensus key hex")?;
    let consensus_key: [u8; 32] = key
        .try_into()
        .map_err(|_| anyhow!("validator consensus key must be 32 bytes"))?;
    Ok(GenesisValidator {
        address: address(addr, "validator address")?,
        consensus_key,
        bond: bond.trim().parse().context("validator bond")?,
    })
}

fn fund(s: &str) -> anyhow::Result<GenesisAccount> {
    let mut parts = s.split(':');
    let (Some(addr), Some(asset), Some(amount), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("--fund must be <address>:<ASSET>:<amount>, got {s:?}");
    };
    Ok(GenesisAccount {
        address: address(addr, "fund address")?,
        asset: Asset::new(asset.trim()),
        amount: amount.trim().parse::<Amount>().context("fund amount")?,
    })
}

/// Apply a JSON object of overrides on top of `params`; `budget` merges.
pub fn merge_params(params: &Params, overrides: &serde_json::Value) -> anyhow::Result<Params> {
    let mut base = serde_json::to_value(params)?;
    let obj = overrides
        .as_object()
        .ok_or_else(|| anyhow!("params overrides must be a JSON object"))?;
    let target = base
        .as_object_mut()
        .ok_or_else(|| anyhow!("params did not serialize to an object"))?;
    for (k, v) in obj {
        if k.starts_with('_') {
            continue;
        }
        match (target.get_mut(k), v) {
            (Some(serde_json::Value::Object(cur)), serde_json::Value::Object(new)) => {
                for (kk, vv) in new {
                    cur.insert(kk.clone(), vv.clone());
                }
            }
            (Some(cur), _) => *cur = v.clone(),
            (None, _) => bail!("unknown parameter {k:?}"),
        }
    }
    let merged: Params = serde_json::from_value(base).context("merged params")?;
    anyhow::ensure!(
        merged.block_timing_ok() && merged.fee_split_ok(),
        "merged parameters fail validation"
    );
    Ok(merged)
}

pub fn build(args: &Args, chain_id: u32) -> anyhow::Result<Genesis> {
    let validators: Vec<GenesisValidator> = args
        .validators
        .iter()
        .map(|s| validator(s))
        .collect::<anyhow::Result<_>>()?;
    let mut g = Genesis::devnet(chain_id, &[], Vec::new());
    let observers = if args.observers.is_empty() {
        validators.iter().map(|v| v.address).collect()
    } else {
        addresses(&args.observers, "observer")?
    };
    let threshold = args
        .observer_threshold
        .unwrap_or((observers.len() as u32 * 2).div_ceil(3).max(1));
    anyhow::ensure!(
        threshold >= 1 && threshold as usize <= observers.len(),
        "observer threshold {threshold} must be between 1 and {}",
        observers.len()
    );
    g.validators = validators;
    g.observers = observers;
    g.observer_threshold = threshold;
    g.arbitrators = addresses(&args.arbitrators, "arbitrator")?;
    g.attesters = addresses(&args.attesters, "attester")?;
    g.param_admin = args
        .param_admin
        .as_deref()
        .map(|s| address(s, "param admin"))
        .transpose()?;
    g.accounts = args
        .funds
        .iter()
        .map(|s| fund(s))
        .collect::<anyhow::Result<_>>()?;
    for a in &g.accounts {
        anyhow::ensure!(
            g.assets.iter().any(|(asset, _, _)| *asset == a.asset),
            "--fund names unknown asset {}",
            a.asset.as_str()
        );
    }
    if let Some(p) = &args.params_json {
        let text = std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
        let overrides: serde_json::Value = serde_json::from_str(&text)?;
        g.params = merge_params(&g.params, &overrides)?;
    }
    if let Some(h) = args.house_operator.as_deref() {
        // The house operator is not part of `Genesis`; it is set by the
        // param admin after launch. Reject the flag until it is.
        bail!("--house-operator {h}: set the house operator through SetParam after launch");
    }
    Ok(g)
}

pub fn run(args: &Args, chain_id: u32) -> anyhow::Result<()> {
    let g = build(args, chain_id)?;
    // Building the state validates the ledger and the allocation.
    let state = g.build();
    let hash = hex::encode(state.compute_hash());
    let json = serde_json::to_string_pretty(&g)?;
    match &args.out {
        Some(p) => {
            std::fs::write(p, json.as_bytes())?;
            eprintln!(
                "wrote {} (chain {}, {} validators, {} observers / threshold {}, state {hash})",
                p.display(),
                g.chain_id,
                g.validators.len(),
                g.observers.len(),
                g.observer_threshold
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use keel_crypto::Keypair;

    fn args() -> Args {
        let v: Vec<String> = (1..=3u64)
            .map(|i| {
                let a = Keypair::from_seed(i).address();
                format!("{}:{}:{}", hex::encode(a.0), a.to_hex(), 100_000_000_000u64)
            })
            .collect();
        let obs: Vec<String> = (1..=3u64)
            .map(|i| Keypair::from_seed(i).address().to_hex())
            .collect();
        Args {
            validators: v,
            observers: obs,
            observer_threshold: Some(2),
            arbitrators: vec![Keypair::from_seed(7).address().to_hex()],
            attesters: vec![Keypair::from_seed(8).address().to_hex()],
            param_admin: Some(Keypair::from_seed(9).address().to_hex()),
            house_operator: None,
            funds: vec![format!(
                "{}:KEEL:{}",
                Keypair::from_seed(9).address().to_hex(),
                5_000_000_000_000u64
            )],
            params_json: None,
            out: None,
        }
    }

    #[test]
    fn roles_are_split_and_state_builds() {
        let g = build(&args(), 3).unwrap();
        assert_eq!(g.chain_id, 3);
        assert_eq!(g.validators.len(), 3);
        assert_eq!(g.observer_threshold, 2);
        assert_eq!(g.arbitrators, vec![Keypair::from_seed(7).address()]);
        assert_eq!(g.attesters, vec![Keypair::from_seed(8).address()]);
        assert_eq!(g.param_admin, Some(Keypair::from_seed(9).address()));
        let state = g.build();
        assert!(state.ledger.audit().mismatches.is_empty());
        assert_eq!(state.staking.validators.len(), 3);
    }

    #[test]
    fn params_merge_and_reject_unknown_keys() {
        let p = Params::default();
        let merged = merge_params(
            &p,
            &serde_json::json!({ "voting_period_blocks": 60, "budget": { "base": 5 }, "_note": 1 }),
        )
        .unwrap();
        assert_eq!(merged.voting_period_blocks, 60);
        assert_eq!(merged.budget.base, 5);
        assert_eq!(merged.budget.max_per_block, p.budget.max_per_block);
        assert!(merge_params(&p, &serde_json::json!({ "nope": 1 })).is_err());
    }

    #[test]
    fn bad_inputs_are_refused() {
        let mut a = args();
        a.observer_threshold = Some(4);
        assert!(build(&a, 3).is_err());
        let mut a = args();
        a.funds = vec!["zz:KEEL:1".into()];
        assert!(build(&a, 3).is_err());
        let mut a = args();
        a.funds = vec![format!(
            "{}:DOGE:1",
            Keypair::from_seed(9).address().to_hex()
        )];
        assert!(build(&a, 3).is_err());
    }
}
