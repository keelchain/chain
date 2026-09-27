//! Chart of accounts and transaction types, as data. The old service kept
//! these as string tables validated at post time; the chain keeps the same
//! names (so a ledger export maps 1:1 into genesis) and adds the accounts a
//! sovereign chain needs: vaults, stable reserve, bonds, treasury, burn.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum NormalSide {
    Credit,
    Debit,
}

/// (type, normal side, restricted). User-facing accounts are credit-normal
/// and restricted (can never go negative). Platform asset accounts are
/// debit-normal and unrestricted.
pub const ACCOUNT_TYPES: &[(&str, NormalSide, bool)] = &[
    // ---- user liabilities (inherited) ----
    ("deposit", NormalSide::Credit, true),
    ("marketplace_escrow", NormalSide::Credit, true),
    ("marketplace_bond", NormalSide::Credit, true),
    ("gift_escrow", NormalSide::Credit, true),
    ("sendout_escrow", NormalSide::Credit, true),
    ("screening_hold", NormalSide::Credit, true),
    // Balances backed by a client's own vault (custody, docs/models.md
    // Model A) and the slice of one on its way out. Restricted and
    // user-owned, but excluded from the network reserve check: each
    // custody vault is checked against its own reserve.
    ("custody", NormalSide::Credit, true),
    ("custody_escrow", NormalSide::Credit, true),
    ("order_escrow", NormalSide::Credit, true),
    // ---- platform revenue (inherited, unrestricted credit-normal) ----
    ("marketplace_escrow_fee", NormalSide::Credit, false),
    ("marketplace_affiliate", NormalSide::Credit, false),
    ("referral_revenue", NormalSide::Credit, false),
    ("expired_referral_revenues", NormalSide::Credit, false),
    ("gift_escrow_fee", NormalSide::Credit, false),
    ("sendout_fee", NormalSide::Credit, false),
    ("sendout_network_fee", NormalSide::Credit, false),
    ("deposit_fee", NormalSide::Credit, false),
    ("internal_transfer_fee", NormalSide::Credit, false),
    ("system_funds", NormalSide::Credit, false),
    ("api_fee", NormalSide::Credit, false),
    ("order_fee", NormalSide::Credit, false),
    // The market maker's inventory: restricted so the ledger refuses any
    // leg the pool cannot cover (2026-08-18). System-owned,
    // so excluded from user liabilities.
    ("swap_pool", NormalSide::Credit, true),
    // ---- platform assets (inherited, debit-normal) ----
    ("hot_wallet", NormalSide::Debit, false),
    ("warm_wallet", NormalSide::Debit, false),
    ("cold_wallet", NormalSide::Debit, false),
    ("fuel_tank", NormalSide::Debit, false),
    ("deposit_addresses", NormalSide::Debit, false),
    ("deposit_incoming", NormalSide::Debit, false),
    ("sweep_gas", NormalSide::Debit, false),
    // ---- chain-native (docs/plan.md §3) ----
    // Coins observed at the TSS vault on the home chain, net of pending
    // outbounds. Debit-normal asset; the reserves side of the block-boundary
    // invariant reserves >= user liabilities.
    ("vault_asset", NormalSide::Debit, false),
    // BTC held hot in observers' Lightning nodes (2026-09-10). Part
    // of reserves like vault_asset; per-observer split lives in the VM.
    ("lightning_pool", NormalSide::Debit, false),
    // Basket assets backing the USD stable 1:1. Restricted credit-normal:
    // the stable can never be minted past what the reserve holds.
    ("stable_reserve", NormalSide::Credit, true),
    // Bonds are the user's money, locked: restricted, user-owned, so they
    // stay inside user liabilities.
    ("stake_bond", NormalSide::Credit, true),
    ("observer_bond", NormalSide::Credit, true),
    ("arbitrator_bond", NormalSide::Credit, true),
    ("offer_deposit", NormalSide::Credit, true),
    ("proposal_deposit", NormalSide::Credit, true),
    // Fee-split destinations. Treasury is restricted so a payout proposal
    // cannot overdraw it; rewards accrue then distribute.
    ("treasury", NormalSide::Credit, true),
    ("validator_rewards", NormalSide::Credit, false),
    ("burn", NormalSide::Credit, false),
    // Native issuance: the KEEL and stable "mint" counter-accounts. Debit-normal
    // so supply is readable as one balance.
    ("issuance", NormalSide::Debit, false),
    // Genesis allocation buckets (docs/tokenomics.md). System-owned,
    // restricted: governance spends them with TreasurySpend-style
    // proposals, nothing can overdraw them.
    ("community_pool", NormalSide::Credit, true),
    ("team_reserve", NormalSide::Credit, true),
    ("validator_bootstrap", NormalSide::Credit, true),
    ("strategic_reserve", NormalSide::Credit, true),
    // KEEL locked under a vesting schedule; released to the beneficiary's
    // deposit by the tokens module.
    ("vesting", NormalSide::Credit, true),
    // KEEL locked for action capacity (2026-09-08) and the slice on
    // its way back to the deposit after an unlock request.
    ("budget_lock", NormalSide::Credit, true),
    ("budget_unlocking", NormalSide::Credit, true),
];

pub fn account_type_info(account_type: &str) -> Option<(NormalSide, bool)> {
    ACCOUNT_TYPES
        .iter()
        .find(|(t, _, _)| *t == account_type)
        .map(|(_, side, restricted)| (*side, *restricted))
}

/// A validated account type name.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
#[serde(transparent)]
pub struct AccountType(String);

impl AccountType {
    pub fn new(name: &str) -> Option<Self> {
        account_type_info(name).map(|_| AccountType(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn info(&self) -> (NormalSide, bool) {
        // Constructed only through `new`, so the lookup cannot fail.
        account_type_info(&self.0).unwrap_or((NormalSide::Credit, true))
    }
}

impl fmt::Display for AccountType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

macro_rules! tx_types {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// Transaction types. The inherited names are kept verbatim so the
        /// ledger export replays into genesis unchanged.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, borsh::BorshSerialize, borsh::BorshDeserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum TxType {
            $($variant),*
        }

        impl TxType {
            pub fn as_str(self) -> &'static str {
                match self { $(TxType::$variant => $name),* }
            }

            pub fn parse(s: &str) -> Option<TxType> {
                match s { $($name => Some(TxType::$variant),)* _ => None }
            }

            pub const ALL: &'static [TxType] = &[$(TxType::$variant),*];
        }
    };
}

tx_types! {
    // marketplace escrow
    MarketplaceEscrowPrepare => "marketplace_escrow_prepare",
    MarketplaceEscrowRelease => "marketplace_escrow_release",
    MarketplaceEscrowCancel => "marketplace_escrow_cancel",
    MarketplaceEscrowAdjust => "marketplace_escrow_adjust",
    MarketplaceBondsDeposit => "marketplace_bonds_deposit",
    MarketplaceBondsWithdraw => "marketplace_bonds_withdraw",
    MarketplaceTradeInternalTransfer => "marketplace_trade_internal_transfer",
    MarketplaceDisputeChargeFee => "marketplace_dispute_charge_fee",
    // wallet edges
    DepositPrepare => "deposit_prepare",
    DepositComplete => "deposit_complete",
    DepositCancel => "deposit_cancel",
    SendoutPrepare => "sendout_prepare",
    SendoutComplete => "sendout_complete",
    SendoutCancel => "sendout_cancel",
    SendoutFailed => "sendout_failed",
    InternalTransferPrepare => "internal_transfer_prepare",
    InternalTransferComplete => "internal_transfer_complete",
    InternalTransferCancel => "internal_transfer_cancel",
    Sweeping => "sweeping",
    Fueling => "fueling",
    Consolidation => "consolidation",
    HotwalletTopup => "hotwallet_topup",
    Rebalancing => "rebalancing",
    SystemFundsDeposit => "system_funds_deposit",
    SystemFundsWithdraw => "system_funds_withdraw",
    SystemFundsIncome => "system_funds_income",
    SystemFundsExpense => "system_funds_expense",
    // gifts
    GiftReserveEscrow => "gift_reserve_escrow",
    GiftReleaseEscrow => "gift_release_escrow",
    GiftReturnEscrow => "gift_return_escrow",
    // deposit screening
    ScreeningHold => "screening_hold",
    ScreeningRelease => "screening_release",
    ScreeningRefund => "screening_refund",
    ScreeningFee => "screening_fee",
    // order book
    OrderLock => "order_lock",
    OrderUnlock => "order_unlock",
    OrderFill => "order_fill",
    Reversal => "reversal",
    // chain-native
    Genesis => "genesis",
    VaultDeposit => "vault_deposit",
    VaultWithdraw => "vault_withdraw",
    VaultMigrate => "vault_migrate",
    StableMint => "stable_mint",
    StableBurn => "stable_burn",
    Bond => "bond",
    Unbond => "unbond",
    Slash => "slash",
    RewardAccrue => "reward_accrue",
    RewardDistribute => "reward_distribute",
    FeeSplit => "fee_split",
    Burn => "burn",
    BudgetPurchase => "budget_purchase",
    OfferDeposit => "offer_deposit",
    OfferRefund => "offer_refund",
    ProposalDeposit => "proposal_deposit",
    ProposalRefund => "proposal_refund",
    TreasuryPayout => "treasury_payout",
    VestingRelease => "vesting_release",
    BudgetLock => "budget_lock",
    BudgetUnlock => "budget_unlock",
    BudgetUnlockRelease => "budget_unlock_release",
    LightningDeposit => "lightning_deposit",
    LightningPayout => "lightning_payout",
    LightningPoolFund => "lightning_pool_fund",
    LightningPoolSweep => "lightning_pool_sweep",
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_account_types_are_all_present() {
        // The exact list from services/ledger/src/domain.rs at the fork.
        for t in [
            "deposit",
            "marketplace_escrow",
            "marketplace_escrow_fee",
            "marketplace_bond",
            "marketplace_affiliate",
            "referral_revenue",
            "expired_referral_revenues",
            "gift_escrow",
            "gift_escrow_fee",
            "sendout_escrow",
            "sendout_fee",
            "sendout_network_fee",
            "deposit_fee",
            "internal_transfer_fee",
            "system_funds",
            "swap_pool",
            "api_fee",
            "hot_wallet",
            "warm_wallet",
            "cold_wallet",
            "fuel_tank",
            "deposit_addresses",
            "deposit_incoming",
            "sweep_gas",
            "screening_hold",
            "order_escrow",
            "order_fee",
        ] {
            assert!(account_type_info(t).is_some(), "missing {t}");
        }
        assert_eq!(
            account_type_info("deposit"),
            Some((NormalSide::Credit, true))
        );
        assert_eq!(
            account_type_info("hot_wallet"),
            Some((NormalSide::Debit, false))
        );
        assert_eq!(account_type_info("nope"), None);
    }

    #[test]
    fn tx_types_round_trip_and_names_are_unique() {
        let mut names: Vec<&str> = TxType::ALL.iter().map(|t| t.as_str()).collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n);
        for t in TxType::ALL {
            assert_eq!(TxType::parse(t.as_str()), Some(*t));
        }
        assert_eq!(TxType::parse("order_fill"), Some(TxType::OrderFill));
    }
}
