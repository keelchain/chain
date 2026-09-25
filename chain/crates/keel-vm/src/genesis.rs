//! Genesis: the state every node starts from. Balances are credited from
//! the `issuance` (native) or `vault_asset` (bridged) system accounts so
//! the books balance from block zero.

use crate::{
    modules::{markets, stable, staking, tokens},
    params::Params,
    state::State,
};
use keel_actions::Chain;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, Amount, Asset, PairConfig};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenesisAccount {
    pub address: Address,
    pub asset: Asset,
    pub amount: Amount,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenesisValidator {
    pub address: Address,
    pub consensus_key: [u8; 32],
    pub bond: Amount,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Genesis {
    pub chain_id: u32,
    pub params: Params,
    /// (asset, decimals, kind)
    pub assets: Vec<(Asset, u32, tokens::AssetKind)>,
    pub accounts: Vec<GenesisAccount>,
    pub pairs: Vec<PairConfig>,
    pub validators: Vec<GenesisValidator>,
    pub observers: Vec<Address>,
    pub observer_threshold: u32,
    pub arbitrators: Vec<Address>,
    pub attesters: Vec<Address>,
    /// (reserve asset, cap)
    pub stable_basket: Vec<(Asset, Amount)>,
    /// Reserve assets already held in the vaults at genesis, backing the
    /// stable balances credited to accounts (migration from a pooled
    /// USDT ledger). Supply must not exceed the converted reserves.
    #[serde(default)]
    pub stable_reserves: Vec<(Asset, Amount)>,
    /// Light-client bootstraps and token contracts (docs/plan.md §4).
    #[serde(default)]
    pub btc_checkpoint: Option<keel_actions::BtcCheckpoint>,
    #[serde(default)]
    pub eth_checkpoint: Option<keel_actions::EthCheckpoint>,
    /// (vault asset, 20-byte contract) for ERC-20 / TRC-20 assets.
    #[serde(default)]
    pub token_contracts: Vec<(Asset, Vec<u8>)>,
    /// KEEL genesis allocation (docs/tokenomics.md). `None` on a
    /// devnet: only the funded accounts hold KEEL.
    #[serde(default)]
    pub allocation: Option<Allocation>,
    /// Address allowed to change parameters directly (`SetParam`) until
    /// governance revokes it: the platform super admin at launch.
    #[serde(default)]
    pub param_admin: Option<Address>,
}

/// How the 21,000,000,000 KEEL are issued at genesis. Shares are basis
/// points of `total_supply`; the awards already promised by the platform
/// (the `accounts` list) come out of the community bucket first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    /// Smallest units (10^-6 KEEL). Default: the 21B hard cap.
    pub total_supply: Amount,
    /// DAO treasury, spendable only by TreasurySpend proposals.
    pub treasury_bps: u32,
    /// Trading, liquidity and referral rewards; existing awards are paid from here.
    pub community_bps: u32,
    /// Team and early contributors, vested to `team`.
    pub team_bps: u32,
    /// Bootstrap bonds and rewards for the first validator/observer sets.
    pub validators_bps: u32,
    /// House-maker inventory (`swap_pool`) and stablecoin liquidity.
    pub liquidity_bps: u32,
    /// Strategic reserve.
    pub reserve_bps: u32,
    /// (address, share of the team bucket in bps). Shares that do not sum
    /// to 10,000 leave the remainder in `team_reserve` for later grants.
    pub team: Vec<(Address, u32)>,
    pub team_cliff_secs: u64,
    pub team_duration_secs: u64,
    /// Block time the schedules start from (genesis block time when 0).
    pub vesting_start_secs: u64,
}

impl Default for Allocation {
    /// Launch proposal: 35% treasury, 25% community, 15% team (1-year cliff,
    /// 4-year linear), 10% validator bootstrap, 10% liquidity, 5% reserve.
    fn default() -> Self {
        Self {
            total_supply: 21_000_000_000 * 1_000_000,
            treasury_bps: 3_500,
            community_bps: 2_500,
            team_bps: 1_500,
            validators_bps: 1_000,
            liquidity_bps: 1_000,
            reserve_bps: 500,
            team: Vec::new(),
            team_cliff_secs: 365 * 86_400,
            team_duration_secs: 4 * 365 * 86_400,
            vesting_start_secs: 0,
        }
    }
}

impl Allocation {
    pub fn bps_ok(&self) -> bool {
        self.treasury_bps
            + self.community_bps
            + self.team_bps
            + self.validators_bps
            + self.liquidity_bps
            + self.reserve_bps
            == 10_000
    }

    fn bucket(&self, bps: u32) -> Amount {
        keel_types::mul_div_floor(self.total_supply, bps as Amount, 10_000).unwrap_or(0)
    }
}

/// Floor-convert an amount between decimal scales.
pub fn convert_decimals(amount: Amount, from: u32, to: u32) -> Amount {
    use keel_types::pow10;
    if from == to {
        amount
    } else if from > to {
        amount / pow10(from - to)
    } else {
        amount.saturating_mul(pow10(to - from))
    }
}

impl Genesis {
    /// A devnet genesis: KEEL + KUSD + vault assets, one pair, funded accounts.
    pub fn devnet(chain_id: u32, funded: &[Address], validators: Vec<GenesisValidator>) -> Self {
        let keel = Asset::new("KEEL");
        let usds = Asset::new("KUSD");
        let btc = Asset::vault(Chain::Bitcoin.as_str(), "BTC");
        let eth_usdt = Asset::vault(Chain::Ethereum.as_str(), "USDT");
        let tron_usdt = Asset::vault(Chain::Tron.as_str(), "USDT");
        let mut accounts = Vec::new();
        for a in funded {
            accounts.push(GenesisAccount {
                address: *a,
                asset: keel.clone(),
                amount: 1_000_000_000_000,
            });
            accounts.push(GenesisAccount {
                address: *a,
                asset: usds.clone(),
                amount: 1_000_000_000_000,
            });
            accounts.push(GenesisAccount {
                address: *a,
                asset: btc.clone(),
                amount: 10_000_000_000,
            });
        }
        Self {
            chain_id,
            params: Params::default(),
            assets: vec![
                (keel.clone(), 6, tokens::AssetKind::Native),
                (usds.clone(), 6, tokens::AssetKind::Stable),
                (
                    btc.clone(),
                    8,
                    tokens::AssetKind::Vault {
                        chain: Chain::Bitcoin,
                    },
                ),
                (
                    Asset::vault("ETH", "ETH"),
                    18,
                    tokens::AssetKind::Vault {
                        chain: Chain::Ethereum,
                    },
                ),
                (
                    eth_usdt.clone(),
                    6,
                    tokens::AssetKind::Vault {
                        chain: Chain::Ethereum,
                    },
                ),
                (
                    Asset::vault("TRON", "TRX"),
                    6,
                    tokens::AssetKind::Vault { chain: Chain::Tron },
                ),
                (
                    tron_usdt.clone(),
                    6,
                    tokens::AssetKind::Vault { chain: Chain::Tron },
                ),
            ],
            accounts,
            pairs: vec![markets::default_pair("BTC-KUSD", btc, usds, 8, 6)],
            observers: validators.iter().map(|v| v.address).collect(),
            observer_threshold: (validators.len() as u32 * 2).div_ceil(3).max(1),
            arbitrators: validators.iter().map(|v| v.address).collect(),
            attesters: validators.iter().map(|v| v.address).collect(),
            validators,
            stable_basket: vec![
                (eth_usdt, 100_000_000_000_000),
                (tron_usdt, 100_000_000_000_000),
            ],
            stable_reserves: Vec::new(),
            btc_checkpoint: None,
            eth_checkpoint: None,
            token_contracts: Vec::new(),
            allocation: None,
            param_admin: None,
        }
    }

    /// Issue the allocation buckets from `issuance`. The KEEL already
    /// credited to accounts (awards, migration) is charged to the community
    /// bucket so the total never exceeds `total_supply`.
    fn apply_allocation(&self, state: &mut State, alloc: &Allocation) {
        assert!(alloc.bps_ok(), "allocation shares must sum to 10000 bps");
        let keel = state.tokens.native.clone();
        let already: Amount = self
            .accounts
            .iter()
            .filter(|a| a.asset == keel)
            .map(|a| a.amount)
            .fold(0, Amount::saturating_add);
        let community = alloc.bucket(alloc.community_bps);
        assert!(
            already <= community,
            "genesis KEEL credits {already} exceed the community bucket {community}"
        );
        let team_total = alloc.bucket(alloc.team_bps);
        let mut team_granted: Amount = 0;
        let start = if alloc.vesting_start_secs == 0 {
            state.timestamp / 1000
        } else {
            alloc.vesting_start_secs
        };
        for (who, share) in &alloc.team {
            let amount =
                keel_types::mul_div_floor(team_total, *share as Amount, 10_000).unwrap_or(0);
            team_granted = team_granted.saturating_add(amount);
            tokens::add_vesting(
                state,
                *who,
                tokens::Vesting {
                    total: amount,
                    released: 0,
                    start_secs: start,
                    cliff_secs: alloc.team_cliff_secs,
                    duration_secs: alloc.team_duration_secs,
                },
            );
        }
        assert!(
            team_granted <= team_total,
            "team shares exceed the team bucket"
        );
        let buckets: [(&str, Amount); 6] = [
            ("treasury", alloc.bucket(alloc.treasury_bps)),
            ("community_pool", community - already),
            ("vesting", team_granted),
            ("team_reserve", team_total - team_granted),
            ("validator_bootstrap", alloc.bucket(alloc.validators_bps)),
            ("swap_pool", alloc.bucket(alloc.liquidity_bps)),
        ];
        let from =
            AccountKey::new(Address::SYSTEM, keel.clone(), "issuance").expect("catalog type");
        let mut records = vec![];
        let mut total: Amount = 0;
        for (t, amount) in buckets
            .iter()
            .chain([("strategic_reserve", alloc.bucket(alloc.reserve_bps))].iter())
        {
            if *amount == 0 {
                continue;
            }
            total = total.saturating_add(*amount);
            records.push(Record::credit(
                AccountKey::new(Address::SYSTEM, keel.clone(), t).expect("catalog type"),
                *amount,
            ));
        }
        records.insert(0, Record::debit(from, total));
        state
            .ledger
            .post(
                "genesis:allocation",
                TxType::Genesis,
                Some("genesis"),
                None,
                records,
            )
            .expect("genesis allocation");
    }

    pub fn build(&self) -> State {
        let mut state = State {
            chain_id: self.chain_id,
            params: self.params.clone(),
            ..Default::default()
        };
        for (asset, decimals, kind) in &self.assets {
            tokens::register(&mut state, asset.clone(), *decimals, kind.clone());
        }
        for (i, a) in self.accounts.iter().enumerate() {
            let source_type = match state.tokens.kind(&a.asset) {
                Some(tokens::AssetKind::Vault { .. }) => "vault_asset",
                _ => "issuance",
            };
            let from = AccountKey::new(Address::SYSTEM, a.asset.clone(), source_type)
                .expect("catalog type");
            let to = AccountKey::new(a.address, a.asset.clone(), "deposit").expect("catalog type");
            state
                .ledger
                .post(
                    &format!("genesis:{i}"),
                    TxType::Genesis,
                    Some("genesis"),
                    None,
                    vec![Record::debit(from, a.amount), Record::credit(to, a.amount)],
                )
                .expect("genesis credit");
        }
        for cfg in &self.pairs {
            markets::list_pair(&mut state, cfg.clone());
        }
        staking::genesis(
            &mut state,
            &self.validators,
            &self.observers,
            self.observer_threshold,
            &self.arbitrators,
        );
        state.gov.param_admin = self.param_admin;
        if let Some(alloc) = &self.allocation {
            self.apply_allocation(&mut state, alloc);
        }
        state.attest.attesters = self.attesters.iter().copied().collect();
        for (asset, cap) in &self.stable_basket {
            stable::set_basket(&mut state, asset.clone(), *cap, true);
        }
        let stable_decimals = state.tokens.decimals(&state.tokens.stable).unwrap_or(6);
        let mut reserves_in_stable: Amount = 0;
        for (i, (asset, amount)) in self.stable_reserves.iter().enumerate() {
            let decimals = state
                .tokens
                .decimals(asset)
                .expect("reserve asset registered");
            let from = AccountKey::new(Address::SYSTEM, asset.clone(), "vault_asset")
                .expect("catalog type");
            let to = AccountKey::new(Address::SYSTEM, asset.clone(), "stable_reserve")
                .expect("catalog type");
            state
                .ledger
                .post(
                    &format!("genesis:reserve:{i}"),
                    TxType::Genesis,
                    Some("genesis"),
                    None,
                    vec![Record::debit(from, *amount), Record::credit(to, *amount)],
                )
                .expect("genesis reserve");
            let e = state.stable.basket.entry(asset.clone()).or_default();
            e.reserve = e.reserve.saturating_add(*amount);
            reserves_in_stable = reserves_in_stable.saturating_add(convert_decimals(
                *amount,
                decimals,
                stable_decimals,
            ));
        }
        if let Some(cp) = &self.btc_checkpoint {
            crate::modules::vaults::apply_btc_checkpoint(&mut state, cp)
                .expect("genesis btc checkpoint");
        }
        if let Some(cp) = &self.eth_checkpoint {
            crate::modules::vaults::apply_eth_checkpoint(&mut state, cp);
        }
        for (asset, contract) in &self.token_contracts {
            assert_eq!(contract.len(), 20, "token contract must be 20 bytes");
            crate::modules::vaults::set_token_contract(&mut state, asset.clone(), contract.clone());
        }
        let stable = state.tokens.stable.clone();
        let supply: Amount = self
            .accounts
            .iter()
            .filter(|a| a.asset == stable)
            .map(|a| a.amount)
            .fold(0, Amount::saturating_add);
        // Stable credited at genesis counts toward supply only when the
        // genesis declares reserves for it (a migration). A devnet genesis
        // funds test accounts with unbacked KUSD and declares none; that
        // supply is outside the reserve invariant by construction.
        if !self.stable_reserves.is_empty() {
            assert!(
                supply <= reserves_in_stable,
                "genesis stable supply {supply} exceeds reserves {reserves_in_stable}"
            );
            state.stable.supply = supply;
        }
        state.last_hash = state.compute_hash();
        state
    }
}
