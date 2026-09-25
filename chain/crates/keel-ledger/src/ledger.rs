//! The posting engine. `post` is the only way a balance changes:
//!   validate -> idempotency gate -> net deltas per account (BTreeMap) ->
//!   check every restricted account stays >= 0 -> commit all balances ->
//!   append to the journal.
//! Nothing is committed unless everything passes, so a failed post leaves
//! the ledger byte-identical.

use std::collections::BTreeMap;

use keel_types::{Address, Amount, Asset, TxSeq};
use serde::{Deserialize, Serialize};

use crate::catalog::{AccountType, NormalSide, TxType};
use crate::error::LedgerError;

/// Identity of an account: who owns it, in which asset, for what purpose.
/// The old ledger's (customer_id, currency, account_type, tenant) key with
/// tenant dropped (one chain, one tenant).
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
pub struct AccountKey {
    pub owner: Address,
    pub asset: Asset,
    pub account_type: AccountType,
}

impl AccountKey {
    pub fn new(owner: Address, asset: impl Into<Asset>, account_type: &str) -> Option<Self> {
        Some(AccountKey {
            owner,
            asset: asset.into(),
            account_type: AccountType::new(account_type)?,
        })
    }

    pub fn is_system(&self) -> bool {
        self.owner.is_system()
    }
}

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
pub struct AccountState {
    /// Signed so unrestricted accounts (revenue, platform assets) may run
    /// negative; restricted ones are refused below zero before commit.
    pub balance: i128,
    pub normal_side: NormalSide,
    pub restricted: bool,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RecordType {
    Debit,
    Credit,
}

impl RecordType {
    pub fn inverse(self) -> RecordType {
        match self {
            RecordType::Debit => RecordType::Credit,
            RecordType::Credit => RecordType::Debit,
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct Record {
    pub account: AccountKey,
    pub record_type: RecordType,
    pub amount: Amount,
}

impl Record {
    pub fn debit(account: AccountKey, amount: Amount) -> Self {
        Record {
            account,
            record_type: RecordType::Debit,
            amount,
        }
    }
    pub fn credit(account: AccountKey, amount: Amount) -> Self {
        Record {
            account,
            record_type: RecordType::Credit,
            amount,
        }
    }
}

/// A committed transaction.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct Posted {
    pub seq: TxSeq,
    pub external_id: String,
    pub tx_type: TxType,
    pub group_id: Option<String>,
    pub reversal_of: Option<TxSeq>,
    pub records: Vec<Record>,
    /// True when the call was an idempotent replay of an existing transaction.
    pub replayed: bool,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Default,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct AuditReport {
    pub accounts_checked: u64,
    /// (account, materialized, recomputed) for every mismatch. Must be empty.
    pub mismatches: Vec<(AccountKey, i128, i128)>,
    pub total_debits: Amount,
    pub total_credits: Amount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Savepoint(usize);

/// Amounts must fit the signed balance with headroom for sums.
const MAX_AMOUNT: Amount = (i128::MAX as Amount) / 4;

#[derive(
    Debug, Clone, Default, Serialize, Deserialize, borsh::BorshSerialize, borsh::BorshDeserialize,
)]
pub struct Ledger {
    accounts: BTreeMap<AccountKey, AccountState>,
    by_external_id: BTreeMap<String, TxSeq>,
    journal: Vec<Posted>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Balance in the account's own normal-side sign; zero when absent.
    pub fn balance(&self, key: &AccountKey) -> i128 {
        self.accounts.get(key).map(|a| a.balance).unwrap_or(0)
    }

    pub fn account(&self, key: &AccountKey) -> Option<&AccountState> {
        self.accounts.get(key)
    }

    pub fn accounts(&self) -> impl Iterator<Item = (&AccountKey, &AccountState)> {
        self.accounts.iter()
    }

    pub fn accounts_of(
        &self,
        owner: Address,
    ) -> impl Iterator<Item = (&AccountKey, &AccountState)> {
        self.accounts.iter().filter(move |(k, _)| k.owner == owner)
    }

    pub fn journal(&self) -> &[Posted] {
        &self.journal
    }

    pub fn next_seq(&self) -> TxSeq {
        TxSeq(self.journal.len() as u64)
    }

    pub fn by_external_id(&self, external_id: &str) -> Option<&Posted> {
        let seq = self.by_external_id.get(external_id)?;
        self.journal.get(seq.0 as usize)
    }

    pub fn by_seq(&self, seq: TxSeq) -> Option<&Posted> {
        self.journal.get(seq.0 as usize)
    }

    /// Post a balanced, single-asset, idempotent transaction.
    pub fn post(
        &mut self,
        external_id: &str,
        tx_type: TxType,
        group_id: Option<&str>,
        reversal_of: Option<TxSeq>,
        records: Vec<Record>,
    ) -> Result<Posted, LedgerError> {
        if external_id.is_empty() {
            return Err(LedgerError::InvalidRequest(
                "external_id is required".into(),
            ));
        }
        if records.is_empty() {
            return Err(LedgerError::InvalidRequest(
                "records must not be empty".into(),
            ));
        }

        // Balance check: sum(debits) == sum(credits), amounts > 0 and bounded.
        let mut debits: Amount = 0;
        let mut credits: Amount = 0;
        for r in &records {
            if r.amount == 0 {
                return Err(LedgerError::InvalidRequest("amount must be > 0".into()));
            }
            if r.amount > MAX_AMOUNT {
                return Err(LedgerError::Overflow);
            }
            match r.record_type {
                RecordType::Debit => {
                    debits = debits.checked_add(r.amount).ok_or(LedgerError::Overflow)?
                }
                RecordType::Credit => {
                    credits = credits.checked_add(r.amount).ok_or(LedgerError::Overflow)?
                }
            }
        }
        if debits != credits {
            return Err(LedgerError::UnbalancedTransaction { debits, credits });
        }

        // Single-asset invariant.
        let asset = &records[0].account.asset;
        if records.iter().any(|r| &r.account.asset != asset) {
            return Err(LedgerError::InvalidRequest(
                "all accounts in a transaction must share one asset".into(),
            ));
        }

        // Idempotency gate: same key must mean same movement.
        if let Some(existing) = self.by_external_id(external_id) {
            if existing.tx_type != tx_type || !records_match(&existing.records, &records) {
                return Err(LedgerError::IdempotencyConflict);
            }
            let mut replay = existing.clone();
            replay.replayed = true;
            return Ok(replay);
        }

        // Net credit-normal delta per account. BTreeMap: deterministic order.
        let mut deltas: BTreeMap<&AccountKey, i128> = BTreeMap::new();
        for r in &records {
            let signed = match r.record_type {
                RecordType::Credit => r.amount as i128,
                RecordType::Debit => -(r.amount as i128),
            };
            let d = deltas.entry(&r.account).or_insert(0);
            *d = d.checked_add(signed).ok_or(LedgerError::Overflow)?;
        }

        // Dry run: compute every new balance, refuse on any violation.
        let mut new_states: Vec<(AccountKey, AccountState)> = Vec::with_capacity(deltas.len());
        for (key, credit_delta) in deltas {
            let (normal_side, restricted) = key.account_type.info();
            let current = self.accounts.get(key).copied().unwrap_or(AccountState {
                balance: 0,
                normal_side,
                restricted,
            });
            let delta = match current.normal_side {
                NormalSide::Credit => credit_delta,
                NormalSide::Debit => -credit_delta,
            };
            let balance = current
                .balance
                .checked_add(delta)
                .ok_or(LedgerError::Overflow)?;
            if current.restricted && balance < 0 {
                return Err(LedgerError::NotEnoughFunds {
                    account: describe(key),
                });
            }
            new_states.push((key.clone(), AccountState { balance, ..current }));
        }

        // Commit.
        for (key, state) in new_states {
            self.accounts.insert(key, state);
        }
        let seq = self.next_seq();
        let posted = Posted {
            seq,
            external_id: external_id.to_string(),
            tx_type,
            group_id: group_id.map(str::to_string),
            reversal_of,
            records,
            replayed: false,
        };
        self.by_external_id.insert(external_id.to_string(), seq);
        self.journal.push(posted.clone());
        Ok(posted)
    }

    /// A point to roll back to: the journal length. Rolling back undoes every
    /// posting after it by applying the inverse deltas, so a multi-post
    /// operation can be made atomic by the caller.
    pub fn savepoint(&self) -> Savepoint {
        Savepoint(self.journal.len())
    }

    pub fn rollback(&mut self, sp: Savepoint) {
        while self.journal.len() > sp.0 {
            let Some(tx) = self.journal.pop() else { break };
            self.by_external_id.remove(&tx.external_id);
            for r in &tx.records {
                if let Some(state) = self.accounts.get_mut(&r.account) {
                    let credit_delta = match r.record_type {
                        RecordType::Credit => -(r.amount as i128),
                        RecordType::Debit => r.amount as i128,
                    };
                    let delta = match state.normal_side {
                        NormalSide::Credit => credit_delta,
                        NormalSide::Debit => -credit_delta,
                    };
                    state.balance = state.balance.saturating_add(delta);
                }
            }
        }
    }

    /// Reverse-by-reference: post the exact inverse of an existing
    /// transaction as a new, idempotent transaction.
    pub fn reverse(
        &mut self,
        original_external_id: &str,
        new_external_id: &str,
        tx_type: TxType,
        group_id: Option<&str>,
    ) -> Result<Posted, LedgerError> {
        let original = self
            .by_external_id(original_external_id)
            .ok_or(LedgerError::TransactionNotFound)?
            .clone();
        let inverse: Vec<Record> = original
            .records
            .iter()
            .map(|r| Record {
                account: r.account.clone(),
                record_type: r.record_type.inverse(),
                amount: r.amount,
            })
            .collect();
        self.post(
            new_external_id,
            tx_type,
            group_id.or(original.group_id.as_deref()),
            Some(original.seq),
            inverse,
        )
    }

    /// What USERS can claim in `asset`: restricted credit-normal balances not
    /// owned by the system. The system's restricted accounts (swap pool,
    /// treasury) are platform money that happens to be overdraw-protected.
    pub fn user_liabilities(&self, asset: &Asset) -> Amount {
        self.accounts
            .iter()
            .filter(|(k, s)| {
                &k.asset == asset
                    && !k.is_system()
                    && s.restricted
                    && s.normal_side == NormalSide::Credit
            })
            .map(|(_, s)| s.balance.max(0) as Amount)
            .fold(0, Amount::saturating_add)
    }

    /// Platform assets in `asset`: sum of debit-normal balances.
    pub fn system_reserves(&self, asset: &Asset) -> i128 {
        self.accounts
            .iter()
            .filter(|(k, s)| &k.asset == asset && s.normal_side == NormalSide::Debit)
            .map(|(_, s)| s.balance)
            .fold(0i128, i128::saturating_add)
    }

    /// Recompute every balance from the journal and compare with the
    /// materialized one. Any mismatch is a bug; this must always be empty.
    pub fn audit(&self) -> AuditReport {
        let mut computed: BTreeMap<&AccountKey, (Amount, Amount)> = BTreeMap::new();
        let mut total_debits: Amount = 0;
        let mut total_credits: Amount = 0;
        for tx in &self.journal {
            for r in &tx.records {
                let e = computed.entry(&r.account).or_insert((0, 0));
                match r.record_type {
                    RecordType::Debit => {
                        e.0 = e.0.saturating_add(r.amount);
                        total_debits = total_debits.saturating_add(r.amount);
                    }
                    RecordType::Credit => {
                        e.1 = e.1.saturating_add(r.amount);
                        total_credits = total_credits.saturating_add(r.amount);
                    }
                }
            }
        }
        let mut mismatches = Vec::new();
        for (key, state) in &self.accounts {
            let (debits, credits) = computed.get(key).copied().unwrap_or((0, 0));
            let recomputed = match state.normal_side {
                NormalSide::Credit => credits as i128 - debits as i128,
                NormalSide::Debit => debits as i128 - credits as i128,
            };
            if recomputed != state.balance {
                mismatches.push((key.clone(), state.balance, recomputed));
            }
        }
        AuditReport {
            accounts_checked: self.accounts.len() as u64,
            mismatches,
            total_debits,
            total_credits,
        }
    }
}

fn describe(key: &AccountKey) -> String {
    format!("{}/{}/{}", key.owner, key.asset, key.account_type)
}

fn records_match(existing: &[Record], new: &[Record]) -> bool {
    if existing.len() != new.len() {
        return false;
    }
    let mut a: Vec<(&AccountKey, RecordType, Amount)> = existing
        .iter()
        .map(|r| (&r.account, r.record_type, r.amount))
        .collect();
    let mut b: Vec<(&AccountKey, RecordType, Amount)> = new
        .iter()
        .map(|r| (&r.account, r.record_type, r.amount))
        .collect();
    a.sort();
    b.sort();
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(owner: u64, asset: &str, t: &str) -> AccountKey {
        AccountKey::new(Address::tagged(owner), asset, t).expect("valid type")
    }
    fn sys(asset: &str, t: &str) -> AccountKey {
        AccountKey::new(Address::SYSTEM, asset, t).expect("valid type")
    }

    /// A deposit: platform asset grew, we owe the user.
    fn deposit(l: &mut Ledger, id: &str, user: u64, amount: Amount) -> Result<Posted, LedgerError> {
        l.post(
            id,
            TxType::DepositComplete,
            None,
            None,
            vec![
                Record::debit(sys("BTC", "vault_asset"), amount),
                Record::credit(key(user, "BTC", "deposit"), amount),
            ],
        )
    }

    #[test]
    fn deposit_then_escrow_then_release_with_fee() {
        let mut l = Ledger::new();
        deposit(&mut l, "dep:1", 1, 1_000_000).expect("deposit");
        assert_eq!(l.balance(&key(1, "BTC", "deposit")), 1_000_000);
        assert_eq!(l.balance(&sys("BTC", "vault_asset")), 1_000_000);

        // Escrow is a transfer, never a lock.
        l.post(
            "trade:1:prepare",
            TxType::MarketplaceEscrowPrepare,
            Some("trade:1"),
            None,
            vec![
                Record::debit(key(1, "BTC", "deposit"), 500_000),
                Record::credit(key(1, "BTC", "marketplace_escrow"), 500_000),
            ],
        )
        .expect("escrow");
        // Release: buyer gets the amount minus fee, fee to revenue.
        l.post(
            "trade:1:release",
            TxType::MarketplaceEscrowRelease,
            Some("trade:1"),
            None,
            vec![
                Record::debit(key(1, "BTC", "marketplace_escrow"), 500_000),
                Record::credit(key(2, "BTC", "deposit"), 495_000),
                Record::credit(sys("BTC", "marketplace_escrow_fee"), 5_000),
            ],
        )
        .expect("release");
        assert_eq!(l.balance(&key(1, "BTC", "marketplace_escrow")), 0);
        assert_eq!(l.balance(&key(2, "BTC", "deposit")), 495_000);
        assert_eq!(l.balance(&sys("BTC", "marketplace_escrow_fee")), 5_000);
        // Liabilities: user 1 has 500k, user 2 495k; fee is not a user claim.
        assert_eq!(l.user_liabilities(&Asset::new("BTC")), 995_000);
        assert_eq!(l.system_reserves(&Asset::new("BTC")), 1_000_000);
        let audit = l.audit();
        assert!(audit.mismatches.is_empty());
        assert_eq!(audit.total_debits, audit.total_credits);
    }

    #[test]
    fn restricted_accounts_never_go_negative_and_nothing_partial_commits() {
        let mut l = Ledger::new();
        deposit(&mut l, "dep:1", 1, 100).expect("deposit");
        let before = l.clone();
        let err = l
            .post(
                "x",
                TxType::InternalTransferComplete,
                None,
                None,
                vec![
                    Record::debit(key(1, "BTC", "deposit"), 101),
                    Record::credit(key(2, "BTC", "deposit"), 101),
                ],
            )
            .unwrap_err();
        assert!(matches!(err, LedgerError::NotEnoughFunds { .. }));
        // Byte-identical: no account touched, no journal entry, no id claimed.
        assert_eq!(l.journal().len(), before.journal().len());
        assert_eq!(l.balance(&key(2, "BTC", "deposit")), 0);
        assert!(l.by_external_id("x").is_none());
        // Unrestricted revenue may go negative (a refund larger than income).
        l.post(
            "refund",
            TxType::Reversal,
            None,
            None,
            vec![
                Record::debit(sys("BTC", "sendout_fee"), 5),
                Record::credit(key(1, "BTC", "deposit"), 5),
            ],
        )
        .expect("unrestricted can go negative");
        assert_eq!(l.balance(&sys("BTC", "sendout_fee")), -5);
    }

    #[test]
    fn unbalanced_multi_asset_and_zero_amounts_are_refused() {
        let mut l = Ledger::new();
        let err = l
            .post(
                "u",
                TxType::DepositComplete,
                None,
                None,
                vec![
                    Record::debit(sys("BTC", "vault_asset"), 10),
                    Record::credit(key(1, "BTC", "deposit"), 9),
                ],
            )
            .unwrap_err();
        assert_eq!(
            err,
            LedgerError::UnbalancedTransaction {
                debits: 10,
                credits: 9
            }
        );
        let err = l
            .post(
                "m",
                TxType::DepositComplete,
                None,
                None,
                vec![
                    Record::debit(sys("BTC", "vault_asset"), 10),
                    Record::credit(key(1, "ETH.USDT", "deposit"), 10),
                ],
            )
            .unwrap_err();
        assert!(matches!(err, LedgerError::InvalidRequest(_)));
        let err = l
            .post(
                "z",
                TxType::DepositComplete,
                None,
                None,
                vec![Record::debit(sys("BTC", "vault_asset"), 0)],
            )
            .unwrap_err();
        assert!(matches!(err, LedgerError::InvalidRequest(_)));
        assert!(l.journal().is_empty());
    }

    #[test]
    fn idempotent_replay_returns_same_tx_and_conflict_is_refused() {
        let mut l = Ledger::new();
        let first = deposit(&mut l, "dep:1", 1, 100).expect("first");
        let again = deposit(&mut l, "dep:1", 1, 100).expect("replay");
        assert!(again.replayed);
        assert_eq!(again.seq, first.seq);
        assert_eq!(l.balance(&key(1, "BTC", "deposit")), 100);
        assert_eq!(l.journal().len(), 1);
        // Same key, different movement: the caller has a bug.
        let err = deposit(&mut l, "dep:1", 1, 101).unwrap_err();
        assert_eq!(err, LedgerError::IdempotencyConflict);
        // Same records in a different order still match.
        let r = l
            .post(
                "dep:1",
                TxType::DepositComplete,
                None,
                None,
                vec![
                    Record::credit(key(1, "BTC", "deposit"), 100),
                    Record::debit(sys("BTC", "vault_asset"), 100),
                ],
            )
            .expect("order-insensitive replay");
        assert!(r.replayed);
    }

    #[test]
    fn reverse_posts_the_exact_inverse() {
        let mut l = Ledger::new();
        deposit(&mut l, "dep:1", 1, 100).expect("deposit");
        l.post(
            "lock",
            TxType::OrderLock,
            Some("order:9"),
            None,
            vec![
                Record::debit(key(1, "BTC", "deposit"), 40),
                Record::credit(key(1, "BTC", "order_escrow"), 40),
            ],
        )
        .expect("lock");
        let rev = l
            .reverse("lock", "unlock", TxType::OrderUnlock, None)
            .expect("reverse");
        assert_eq!(rev.reversal_of, Some(TxSeq(1)));
        assert_eq!(rev.group_id.as_deref(), Some("order:9"));
        assert_eq!(l.balance(&key(1, "BTC", "deposit")), 100);
        assert_eq!(l.balance(&key(1, "BTC", "order_escrow")), 0);
        assert_eq!(
            l.reverse("missing", "x", TxType::Reversal, None)
                .unwrap_err(),
            LedgerError::TransactionNotFound
        );
        assert!(l.audit().mismatches.is_empty());
    }

    #[test]
    fn rollback_restores_balances_and_ids() {
        let mut l = Ledger::new();
        deposit(&mut l, "dep:1", 1, 100).expect("deposit");
        let before = l.clone();
        let sp = l.savepoint();
        l.post(
            "lock",
            TxType::OrderLock,
            None,
            None,
            vec![
                Record::debit(key(1, "BTC", "deposit"), 40),
                Record::credit(key(1, "BTC", "order_escrow"), 40),
            ],
        )
        .expect("lock");
        l.post(
            "fee",
            TxType::OrderFill,
            None,
            None,
            vec![
                Record::debit(key(1, "BTC", "order_escrow"), 1),
                Record::credit(sys("BTC", "treasury"), 1),
            ],
        )
        .expect("fee");
        l.rollback(sp);
        assert_eq!(l.balance(&key(1, "BTC", "deposit")), 100);
        assert_eq!(l.balance(&key(1, "BTC", "order_escrow")), 0);
        assert_eq!(l.balance(&sys("BTC", "treasury")), 0);
        assert!(l.by_external_id("lock").is_none());
        assert_eq!(l.journal().len(), before.journal().len());
        assert!(l.audit().mismatches.is_empty());
        // The id can be reused after rollback.
        deposit(&mut l, "lock", 2, 5).expect("reuse");
    }

    #[test]
    fn same_account_on_both_sides_nets_out() {
        let mut l = Ledger::new();
        deposit(&mut l, "dep:1", 1, 10).expect("deposit");
        // Debit 10 and credit 10 on the same account: net zero, allowed.
        l.post(
            "noop",
            TxType::Rebalancing,
            None,
            None,
            vec![
                Record::debit(key(1, "BTC", "deposit"), 10),
                Record::credit(key(1, "BTC", "deposit"), 10),
            ],
        )
        .expect("net zero");
        assert_eq!(l.balance(&key(1, "BTC", "deposit")), 10);
    }

    #[test]
    fn swap_pool_and_treasury_are_not_user_liabilities() {
        let mut l = Ledger::new();
        l.post(
            "seed",
            TxType::SystemFundsDeposit,
            None,
            None,
            vec![
                Record::debit(sys("KUSD", "vault_asset"), 1_000),
                Record::credit(sys("KUSD", "swap_pool"), 600),
                Record::credit(sys("KUSD", "treasury"), 400),
            ],
        )
        .expect("seed");
        assert_eq!(l.user_liabilities(&Asset::new("KUSD")), 0);
        // The pool is restricted: it cannot pay what it does not hold.
        let err = l
            .post(
                "over",
                TxType::OrderFill,
                None,
                None,
                vec![
                    Record::debit(sys("KUSD", "swap_pool"), 601),
                    Record::credit(key(1, "KUSD", "deposit"), 601),
                ],
            )
            .unwrap_err();
        assert!(matches!(err, LedgerError::NotEnoughFunds { .. }));
    }
}
