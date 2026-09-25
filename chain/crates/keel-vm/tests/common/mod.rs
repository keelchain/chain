#![allow(dead_code, clippy::unwrap_used)]
//! Shared helpers for the staking / governance / stable integration tests.

use keel_actions::{Action, SignedAction, CHAIN_ID_DEVNET};
use keel_crypto::Keypair;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, Asset};
use keel_vm::{
    apply_block, genesis::GenesisValidator, BlockContext, Event, Genesis, Receipt, State,
};

pub struct Actor {
    pub key: Keypair,
    pub nonce: u64,
}

impl Actor {
    pub fn new(seed: u64) -> Self {
        Self {
            key: Keypair::from_seed(seed),
            nonce: 0,
        }
    }
    pub fn from_keypair(key: Keypair) -> Self {
        Self { key, nonce: 0 }
    }
    pub fn addr(&self) -> Address {
        self.key.address()
    }
    pub fn act(&mut self, a: Action) -> SignedAction {
        let s = SignedAction::sign(&self.key, self.nonce, CHAIN_ID_DEVNET, a);
        self.nonce += 1;
        s
    }
}

pub const T0: u64 = 1_700_000_000;
pub const VALIDATOR_SEED: u64 = 100;

/// Devnet genesis with alice (seed 21), bob (seed 22) funded and one genesis
/// validator (seed 100, bonded `validator_bond`).
pub fn setup(validator_bond: u128) -> (State, Actor, Actor, Actor) {
    let alice = Actor::new(21);
    let bob = Actor::new(22);
    let validator = Actor::new(VALIDATOR_SEED);
    let g = Genesis::devnet(
        CHAIN_ID_DEVNET,
        &[alice.addr(), bob.addr(), validator.addr()],
        vec![GenesisValidator {
            address: validator.addr(),
            consensus_key: validator.addr().0,
            bond: validator_bond,
        }],
    );
    (g.build(), alice, bob, validator)
}

pub fn at(height: u64) -> BlockContext {
    BlockContext {
        height,
        timestamp: (T0 + height) * 1000,
        proposer: None,
    }
}

/// Applies blocks with an increasing height.
pub struct Chain {
    pub state: State,
    pub height: u64,
}

impl Chain {
    pub fn new(state: State) -> Self {
        Self { state, height: 0 }
    }
    /// Apply one block with `actions`; returns receipts and end-block events.
    pub fn block(&mut self, actions: &[SignedAction]) -> (Vec<Receipt>, Vec<Event>) {
        self.height += 1;
        apply_block(&mut self.state, &at(self.height), actions)
    }
    /// Apply one block whose BFT time is `secs` (for time-driven logic such
    /// as vesting and payment windows).
    pub fn block_at_secs(
        &mut self,
        secs: u64,
        actions: &[SignedAction],
    ) -> (Vec<Receipt>, Vec<Event>) {
        self.height += 1;
        let ctx = BlockContext {
            height: self.height,
            timestamp: secs * 1000,
            proposer: None,
        };
        apply_block(&mut self.state, &ctx, actions)
    }
    /// Apply empty blocks until `height` is reached (inclusive).
    pub fn advance_to(&mut self, height: u64) -> Vec<Event> {
        let mut events = Vec::new();
        while self.height < height {
            events.extend(self.block(&[]).1);
        }
        events
    }
}

pub fn keel() -> Asset {
    Asset::new("KEEL")
}
pub fn usds() -> Asset {
    Asset::new("KUSD")
}
pub fn btc() -> Asset {
    Asset::new("BTC.BTC")
}

pub fn sys(state: &State, asset: &Asset, t: &str) -> i128 {
    state
        .ledger
        .balance(&AccountKey::new(Address::SYSTEM, asset.clone(), t).unwrap())
}

pub fn acct(state: &State, who: Address, asset: &Asset, t: &str) -> i128 {
    state
        .ledger
        .balance(&AccountKey::new(who, asset.clone(), t).unwrap())
}

pub fn ok(r: &Receipt) {
    assert!(r.ok, "{:?}", r.error);
}

pub fn err(r: &Receipt) -> &keel_vm::VmError {
    assert!(!r.ok, "expected failure, got ok");
    r.error.as_ref().unwrap()
}

/// Credit `who` with `amount` of a vault asset straight from reserves.
pub fn fund_vault_asset(state: &mut State, who: Address, asset: &Asset, amount: u128, tag: &str) {
    state
        .ledger
        .post(
            &format!("fund:{tag}"),
            TxType::VaultDeposit,
            None,
            None,
            vec![
                Record::debit(
                    AccountKey::new(Address::SYSTEM, asset.clone(), "vault_asset").unwrap(),
                    amount,
                ),
                Record::credit(
                    AccountKey::new(who, asset.clone(), "deposit").unwrap(),
                    amount,
                ),
            ],
        )
        .unwrap();
}

/// Put `amount` of `asset` into a system credit-normal account from issuance.
pub fn fund_system(state: &mut State, asset: &Asset, account_type: &str, amount: u128, tag: &str) {
    state
        .ledger
        .post(
            &format!("fund-sys:{tag}"),
            TxType::SystemFundsDeposit,
            None,
            None,
            vec![
                Record::debit(
                    AccountKey::new(Address::SYSTEM, asset.clone(), "issuance").unwrap(),
                    amount,
                ),
                Record::credit(
                    AccountKey::new(Address::SYSTEM, asset.clone(), account_type).unwrap(),
                    amount,
                ),
            ],
        )
        .unwrap();
}

pub fn audit_clean(state: &State) {
    assert!(state.ledger.audit().mismatches.is_empty());
}
