use keel_types::Amount;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LedgerError {
    #[error("unknown account type: {0}")]
    AccountTypeNotFound(String),
    #[error("transaction not found")]
    TransactionNotFound,
    #[error("transaction records do not balance: debits={debits} credits={credits}")]
    UnbalancedTransaction { debits: Amount, credits: Amount },
    #[error("not enough funds in account {account}")]
    NotEnoughFunds { account: String },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("external_id already used with different payload")]
    IdempotencyConflict,
    #[error("amount overflow")]
    Overflow,
}

impl LedgerError {
    pub fn code(&self) -> &'static str {
        match self {
            LedgerError::AccountTypeNotFound(_) => "ACCOUNT_TYPE_NOT_FOUND",
            LedgerError::TransactionNotFound => "TRANSACTION_NOT_FOUND",
            LedgerError::UnbalancedTransaction { .. } => "UNBALANCED_TRANSACTION",
            LedgerError::NotEnoughFunds { .. } => "NOT_ENOUGH_FUNDS",
            LedgerError::InvalidRequest(_) => "INVALID_REQUEST",
            LedgerError::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            LedgerError::Overflow => "OVERFLOW",
        }
    }
}
