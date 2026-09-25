//! `keel genesis-from-export`: turn the marketplace ledger's account export
//! into a chain genesis (docs/migration.md step 4).
//!
//! Input: a CSV with `customer_id,currency,account_type,balance` (the
//! `accounts` table), a JSON map `customer_id -> chain address hex`, a JSON
//! map of vault reserves per chain asset, and the validator list. Output:
//! `keel_vm::Genesis` as JSON.
//!
//! Rules:
//! - Only user liabilities migrate (restricted credit-normal accounts of
//!   non-system customers); platform revenue and asset accounts do not.
//! - Escrow balances are refused unless `--merge-escrow`: the cut-over
//!   playbook requires open trades to be settled first.
//! - Pooled `USDT`/`USDC` become the stable coin 1:1; the reserves map must
//!   cover the resulting supply (checked by `Genesis::build`).
//! - Unknown currencies abort unless `--skip-unmapped` (then reported).

use anyhow::{anyhow, bail, Context as _};
use keel_types::{Address, Amount, Asset};
use keel_vm::{
    genesis::{Genesis, GenesisAccount, GenesisValidator},
    Params,
};
use std::collections::BTreeMap;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// CSV: customer_id,currency,account_type,balance
    #[arg(long)]
    pub export: String,
    /// JSON object: customer_id -> chain address (hex, 32 bytes)
    #[arg(long)]
    pub addresses: String,
    /// JSON object: chain asset -> amount held in the vault, e.g. {"ETH.USDT": "1000000"}
    #[arg(long)]
    pub reserves: Option<String>,
    /// JSON array of {address, consensus_key, bond}
    #[arg(long)]
    pub validators: String,
    #[arg(long)]
    pub out: String,
    /// Chain address (hex) of the platform super admin: may change
    /// parameters directly until governance revokes it.
    #[arg(long)]
    pub param_admin: Option<String>,
    /// JSON file with the KEEL allocation (keel_vm::genesis::Allocation);
    /// omitted = the tokenomics defaults with no team grants.
    #[arg(long)]
    pub allocation: Option<String>,
    /// Skip the KEEL allocation entirely (test networks).
    #[arg(long)]
    pub no_allocation: bool,
    #[arg(long)]
    pub merge_escrow: bool,
    #[arg(long)]
    pub skip_unmapped: bool,
}

const LIABILITY_TYPES: &[&str] = &[
    "deposit",
    "marketplace_escrow",
    "marketplace_bond",
    "gift_escrow",
    "sendout_escrow",
    "screening_hold",
    "order_escrow",
];
const ESCROW_TYPES: &[&str] = &[
    "marketplace_escrow",
    "gift_escrow",
    "sendout_escrow",
    "screening_hold",
    "order_escrow",
];

/// Ledger currency code -> chain asset.
pub fn map_currency(code: &str) -> Option<Asset> {
    match code {
        "BTC" => Some(Asset::new("BTC.BTC")),
        "ETH" => Some(Asset::new("ETH.ETH")),
        "TRX" => Some(Asset::new("TRON.TRX")),
        "USDT" | "USDC" | "USDT-ERC20" | "USDT-TRC20" => Some(Asset::new("KUSD")),
        "KEEL" => Some(Asset::new("KEEL")),
        _ => None,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub accounts: usize,
    pub totals: BTreeMap<Asset, Amount>,
    pub skipped_currencies: BTreeMap<String, Amount>,
    pub system_rows: usize,
}

/// Convert CSV rows into genesis accounts.
pub fn convert(
    csv: &str,
    addresses: &BTreeMap<String, Address>,
    merge_escrow: bool,
    skip_unmapped: bool,
) -> anyhow::Result<(Vec<GenesisAccount>, Summary)> {
    let mut per: BTreeMap<(Address, Asset), Amount> = BTreeMap::new();
    let mut summary = Summary::default();
    for (n, line) in csv.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || (n == 0 && line.starts_with("customer_id")) {
            continue;
        }
        let cols: Vec<&str> = line.split(',').map(str::trim).collect();
        if cols.len() < 4 {
            bail!("line {}: expected 4 columns", n + 1);
        }
        let (customer, currency, account_type, balance) = (cols[0], cols[1], cols[2], cols[3]);
        if customer == "system" {
            summary.system_rows += 1;
            continue;
        }
        if !LIABILITY_TYPES.contains(&account_type) {
            continue;
        }
        let balance: i128 = balance
            .parse()
            .with_context(|| format!("line {}: balance", n + 1))?;
        if balance <= 0 {
            continue;
        }
        if ESCROW_TYPES.contains(&account_type) && !merge_escrow {
            bail!("line {}: {customer} has {balance} in {account_type}; settle open trades or pass --merge-escrow", n + 1);
        }
        let Some(asset) = map_currency(currency) else {
            if skip_unmapped {
                *summary
                    .skipped_currencies
                    .entry(currency.to_string())
                    .or_insert(0) += balance as Amount;
                continue;
            }
            bail!("line {}: unmapped currency {currency}", n + 1);
        };
        let address = *addresses
            .get(customer)
            .ok_or_else(|| anyhow!("line {}: no chain address for customer {customer}", n + 1))?;
        *per.entry((address, asset)).or_insert(0) += balance as Amount;
    }
    let mut accounts = Vec::with_capacity(per.len());
    for ((address, asset), amount) in per {
        *summary.totals.entry(asset.clone()).or_insert(0) += amount;
        accounts.push(GenesisAccount {
            address,
            asset,
            amount,
        });
    }
    summary.accounts = accounts.len();
    Ok((accounts, summary))
}

pub fn run(args: &Args, chain_id: u32) -> anyhow::Result<()> {
    let csv = std::fs::read_to_string(&args.export)?;
    let addresses: BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(&args.addresses)?)?;
    let addresses: BTreeMap<String, Address> = addresses
        .into_iter()
        .map(|(k, v)| match Address::from_hex(&v) {
            Some(a) => Ok((k, a)),
            None => Err(anyhow!("bad address for {k}")),
        })
        .collect::<Result<_, _>>()?;
    let validators: Vec<GenesisValidator> =
        serde_json::from_str(&std::fs::read_to_string(&args.validators)?)?;
    let reserves: BTreeMap<String, String> = match &args.reserves {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?,
        None => BTreeMap::new(),
    };
    let (accounts, summary) = convert(&csv, &addresses, args.merge_escrow, args.skip_unmapped)?;

    let mut genesis = Genesis::devnet(chain_id, &[], validators);
    genesis.params = Params::default();
    genesis.accounts = accounts;
    genesis.stable_reserves = reserves
        .into_iter()
        .map(|(a, v)| {
            v.parse::<Amount>()
                .map(|n| (Asset::new(a), n))
                .map_err(|e| anyhow!("reserve {e}"))
        })
        .collect::<Result<_, _>>()?;
    genesis.param_admin = match &args.param_admin {
        Some(hex) => Some(
            Address::from_hex(hex).ok_or_else(|| anyhow!("--param-admin must be 64 hex chars"))?,
        ),
        None => None,
    };
    genesis.allocation = if args.no_allocation {
        None
    } else {
        match &args.allocation {
            Some(p) => Some(serde_json::from_str(&std::fs::read_to_string(p)?)?),
            None => Some(keel_vm::genesis::Allocation::default()),
        }
    };
    if let Some(a) = &genesis.allocation {
        if !a.bps_ok() {
            bail!("allocation shares must sum to 10000 bps");
        }
    }
    // Building verifies the ledger balances, the stable reserve cover and
    // that promised KEEL awards fit the community bucket.
    let state = genesis.build();
    let audit = state.ledger.audit();
    if !audit.mismatches.is_empty() {
        bail!(
            "genesis ledger audit failed: {} mismatches",
            audit.mismatches.len()
        );
    }
    std::fs::write(&args.out, serde_json::to_vec_pretty(&genesis)?)?;
    eprintln!(
        "wrote {} ({} accounts, {} system rows skipped)",
        args.out, summary.accounts, summary.system_rows
    );
    for (asset, total) in &summary.totals {
        eprintln!("  {asset}: {total}");
    }
    for (cur, total) in &summary.skipped_currencies {
        eprintln!("  skipped {cur}: {total}");
    }
    eprintln!("state hash {}", hex::encode(state.last_hash));
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn addrs() -> BTreeMap<String, Address> {
        [
            ("u1".to_string(), Address::tagged(1)),
            ("u2".to_string(), Address::tagged(2)),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn maps_liabilities_and_pools_usdt_into_usds() {
        let csv = "customer_id,currency,account_type,balance\n\
                   u1,BTC,deposit,150000000\n\
                   u1,USDT,deposit,2000000\n\
                   u2,USDT,deposit,3000000\n\
                   u2,USDT,deposit,-5\n\
                   system,BTC,hot_wallet,999\n\
                   system,USDT,marketplace_escrow_fee,42\n\
                   u1,BTC,referral_revenue,7\n";
        let (accounts, summary) = convert(csv, &addrs(), false, false).unwrap();
        assert_eq!(accounts.len(), 3);
        assert_eq!(summary.totals[&Asset::new("BTC.BTC")], 150_000_000);
        assert_eq!(summary.totals[&Asset::new("KUSD")], 5_000_000);
        assert_eq!(summary.system_rows, 2);
        // Builds into a valid genesis when reserves cover the stable supply.
        let mut g = Genesis::devnet(1, &[], vec![]);
        g.accounts = accounts;
        g.stable_reserves = vec![(Asset::new("ETH.USDT"), 5_000_000)];
        let state = g.build();
        assert!(state.ledger.audit().mismatches.is_empty());
        assert_eq!(state.stable.supply, 5_000_000);
    }

    #[test]
    fn escrow_and_unmapped_are_refused_unless_flagged() {
        let csv = "u1,BTC,marketplace_escrow,10\n";
        assert!(convert(csv, &addrs(), false, false).is_err());
        let (a, _) = convert(csv, &addrs(), true, false).unwrap();
        assert_eq!(a[0].amount, 10);
        let csv = "u1,DOGE,deposit,10\n";
        assert!(convert(csv, &addrs(), false, false).is_err());
        let (a, s) = convert(csv, &addrs(), false, true).unwrap();
        assert!(a.is_empty());
        assert_eq!(s.skipped_currencies["DOGE"], 10);
        assert!(convert("u9,BTC,deposit,1\n", &addrs(), false, false).is_err());
    }

    #[test]
    #[should_panic(expected = "exceeds reserves")]
    fn stable_supply_above_reserves_is_refused() {
        let mut g = Genesis::devnet(1, &[], vec![]);
        g.accounts = vec![GenesisAccount {
            address: Address::tagged(1),
            asset: Asset::new("KUSD"),
            amount: 10,
        }];
        g.stable_reserves = vec![(Asset::new("ETH.USDT"), 5)];
        g.build();
    }
}
