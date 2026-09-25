#![allow(clippy::unwrap_used)]
//! End-to-end VM tests for the exchange core.

use keel_actions::{Action, HouseQuote, PlaceOrder, SignedAction, Transfer, CHAIN_ID_DEVNET};
use keel_crypto::Keypair;
use keel_ledger::AccountKey;
use keel_types::{Address, Asset, OrderId, OrderType, Side};
use keel_vm::{
    apply_block, genesis::GenesisValidator, modules::tokens, BlockContext, Genesis, State, VmError,
};

struct Actor {
    key: Keypair,
    nonce: u64,
}

impl Actor {
    fn new(seed: u64) -> Self {
        Self {
            key: Keypair::from_seed(seed),
            nonce: 0,
        }
    }
    fn addr(&self) -> Address {
        self.key.address()
    }
    fn act(&mut self, a: Action) -> SignedAction {
        let s = SignedAction::sign(&self.key, self.nonce, CHAIN_ID_DEVNET, a);
        self.nonce += 1;
        s
    }
}

fn setup() -> (State, Actor, Actor) {
    let alice = Actor::new(1);
    let bob = Actor::new(2);
    let validator = Keypair::from_seed(100);
    let g = Genesis::devnet(
        CHAIN_ID_DEVNET,
        &[alice.addr(), bob.addr()],
        vec![GenesisValidator {
            address: validator.address(),
            consensus_key: validator.address().0,
            bond: 0,
        }],
    );
    (g.build(), alice, bob)
}

fn ctx(height: u64) -> BlockContext {
    BlockContext {
        height,
        timestamp: 1_700_000_000_000 + height * 250,
        proposer: None,
    }
}

fn btc() -> Asset {
    Asset::new("BTC.BTC")
}
fn usds() -> Asset {
    Asset::new("KUSD")
}
fn px(usd: u128) -> u128 {
    usd * 1_000_000
}

#[test]
fn genesis_balances_and_transfer() {
    let (mut state, mut alice, bob) = setup();
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        10_000_000_000
    );
    let h0 = state.last_hash;
    let (r, _) = apply_block(
        &mut state,
        &ctx(1),
        &[alice.act(Action::Transfer(Transfer {
            to: bob.addr(),
            asset: usds(),
            amount: 5_000_000,
            memo: None,
        }))],
    );
    assert!(r[0].ok, "{:?}", r[0].error);
    assert_eq!(
        tokens::balance(&state, bob.addr(), &usds()),
        1_000_000_000_000 + 5_000_000
    );
    assert_ne!(state.last_hash, h0);
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn admission_rejects_replay_bad_sig_and_wrong_chain() {
    let (mut state, mut alice, bob) = setup();
    let t = Action::Transfer(Transfer {
        to: bob.addr(),
        asset: usds(),
        amount: 1,
        memo: None,
    });
    let first = alice.act(t.clone());
    let mut forged = first.clone();
    forged.envelope.action = Action::Transfer(Transfer {
        to: bob.addr(),
        asset: usds(),
        amount: 2,
        memo: None,
    });
    let wrong_chain = SignedAction::sign(&alice.key, 1, 99, t.clone());
    let (r, _) = apply_block(
        &mut state,
        &ctx(1),
        &[first.clone(), first.clone(), forged, wrong_chain],
    );
    assert!(r[0].ok);
    assert_eq!(
        r[1].error,
        Some(VmError::BadNonce {
            expected: 1,
            got: 0
        })
    );
    assert_eq!(r[2].error, Some(VmError::BadSignature));
    assert_eq!(r[3].error, Some(VmError::WrongChain));
    // A failed action still consumed its nonce? Only when admitted: the
    // replay failed before nonce consumption, so nonce is 1.
    assert_eq!(state.account_ref(&alice.addr()).unwrap().nonce, 1);
}

#[test]
fn limit_orders_cross_settle_and_split_fees() {
    let (mut state, mut alice, mut bob) = setup();
    // Alice sells 1 BTC at 60,000; Bob buys 0.5 BTC at 60,010 (gets Alice's price).
    let sell = alice.act(Action::PlaceOrder(PlaceOrder {
        pair: "BTC-KUSD".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(px(60_000)),
        quantity: Some(100_000_000),
        quote_budget: None,
        client_id: Some(1),
    }));
    let buy = bob.act(Action::PlaceOrder(PlaceOrder {
        pair: "BTC-KUSD".into(),
        side: Side::Buy,
        order_type: OrderType::Limit,
        price: Some(px(60_010)),
        quantity: Some(50_000_000),
        quote_budget: None,
        client_id: Some(2),
    }));
    let (r, _) = apply_block(&mut state, &ctx(1), &[sell, buy]);
    assert!(r[0].ok, "{:?}", r[0].error);
    assert!(r[1].ok, "{:?}", r[1].error);

    // Alice: 1 BTC locked, 0.5 filled -> deposit 99.0 BTC, escrow 0.5 BTC, +30,000 KUSD.
    assert_eq!(tokens::balance(&state, alice.addr(), &btc()), 9_900_000_000);
    let alice_escrow = AccountKey::new(alice.addr(), btc(), "order_escrow").unwrap();
    assert_eq!(state.ledger.balance(&alice_escrow), 50_000_000);
    assert_eq!(
        tokens::balance(&state, alice.addr(), &usds()),
        1_000_000_000_000 + px(30_000)
    );
    // Bob: paid 30,000 (locked 30,005, improvement released), received 0.5 BTC minus 10 bps fee.
    assert_eq!(
        tokens::balance(&state, bob.addr(), &usds()),
        1_000_000_000_000 - px(30_000)
    );
    let fee = 50_000_000 * 10 / 10_000;
    assert_eq!(
        tokens::balance(&state, bob.addr(), &btc()),
        10_000_000_000 + 50_000_000 - fee
    );
    // Fee split 50/40/10 lands in system accounts.
    let sys = |t: &str| {
        state
            .ledger
            .balance(&AccountKey::new(Address::SYSTEM, btc(), t).unwrap())
    };
    assert_eq!(sys("treasury"), (fee / 2) as i128);
    assert_eq!(sys("validator_rewards"), (fee * 4 / 10) as i128);
    assert_eq!(sys("burn"), (fee / 10) as i128);
    // Book: Alice's remainder rests; last price set; USD valuation works.
    let m = &state.markets.pairs["BTC-KUSD"];
    assert_eq!(m.book.best_price(Side::Sell), Some(px(60_000)));
    assert_eq!(m.last_price, Some(px(60_000)));
    assert_eq!(
        tokens::usd_value(&state, &btc(), 100_000_000),
        Some(px(60_000))
    );
    // Budgets grew with volume: 30,000 USD filled -> +30,000 actions each.
    assert_eq!(
        state.account_ref(&bob.addr()).unwrap().budget.earned,
        30_000
    );
    assert!(state.ledger.audit().mismatches.is_empty());

    // Cancel Alice's remainder: escrow back to deposit.
    let (r, _) = apply_block(
        &mut state,
        &ctx(2),
        &[alice.act(Action::CancelOrder {
            order_id: OrderId(0),
        })],
    );
    assert!(r[0].ok, "{:?}", r[0].error);
    assert_eq!(state.ledger.balance(&alice_escrow), 0);
    assert_eq!(tokens::balance(&state, alice.addr(), &btc()), 9_950_000_000);
    assert!(state.markets.pairs["BTC-KUSD"].book.is_empty());
    // Bob cannot cancel Alice's order; a second cancel is refused.
    let (r, _) = apply_block(
        &mut state,
        &ctx(3),
        &[bob.act(Action::CancelOrder {
            order_id: OrderId(0),
        })],
    );
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
}

#[test]
fn insufficient_funds_leaves_state_untouched() {
    let (mut state, mut alice, _bob) = setup();
    let before = state.compute_hash();
    let (r, _) = apply_block(
        &mut state,
        &ctx(1),
        &[alice.act(Action::PlaceOrder(PlaceOrder {
            pair: "BTC-KUSD".into(),
            side: Side::Sell,
            order_type: OrderType::Limit,
            price: Some(px(60_000)),
            quantity: Some(20_000_000_000), // 200 BTC, has 100
            quote_budget: None,
            client_id: None,
        }))],
    );
    assert_eq!(r[0].error, Some(VmError::NotEnoughFunds));
    // Nonce and budget were consumed; nothing else changed.
    let meta = state.account_ref(&alice.addr()).unwrap();
    assert_eq!(meta.nonce, 1);
    assert_eq!(meta.budget.used, 1);
    assert!(state.markets.orders.is_empty());
    assert!(state.ledger.audit().mismatches.is_empty());
    let mut probe = state.clone();
    probe.accounts.remove(&alice.addr());
    let mut base = State::restore(&state.snapshot()).unwrap();
    base.accounts.remove(&alice.addr());
    assert_eq!(probe.compute_hash(), base.compute_hash());
    assert_ne!(before, state.compute_hash());
}

#[test]
fn market_buy_by_budget_and_house_quote() {
    let (mut state, mut alice, mut bob) = setup();
    // Nobody may quote for the system house until governance sets an operator.
    let q = HouseQuote {
        pair: "BTC-KUSD".into(),
        bid: Some((px(59_000), 100_000_000)),
        ask: Some((px(61_000), 100_000_000)),
        valid_until: 10,
    };
    let (r, _) = apply_block(
        &mut state,
        &ctx(1),
        &[alice.act(Action::HouseQuote(q.clone()))],
    );
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
    state.gov_house_operator = Some(alice.addr());
    // Fund the house inventory from genesis-style issuance.
    state
        .ledger
        .post(
            "seed:house",
            keel_ledger::TxType::SystemFundsDeposit,
            None,
            None,
            vec![
                keel_ledger::Record::debit(
                    AccountKey::new(Address::SYSTEM, btc(), "vault_asset").unwrap(),
                    100_000_000,
                ),
                keel_ledger::Record::credit(
                    AccountKey::new(Address::SYSTEM, btc(), "swap_pool").unwrap(),
                    100_000_000,
                ),
            ],
        )
        .unwrap();
    let (r, _) = apply_block(&mut state, &ctx(2), &[alice.act(Action::HouseQuote(q))]);
    assert!(r[0].ok, "{:?}", r[0].error);
    // Bob market-buys with a 1,000 KUSD budget: fills the house ask at 61,000.
    let (r, _) = apply_block(
        &mut state,
        &ctx(3),
        &[bob.act(Action::PlaceOrder(PlaceOrder {
            pair: "BTC-KUSD".into(),
            side: Side::Buy,
            order_type: OrderType::Market,
            price: None,
            quantity: None,
            quote_budget: Some(px(1_000)),
            client_id: None,
        }))],
    );
    assert!(r[0].ok, "{:?}", r[0].error);
    // 1000/61000 BTC = 1,639,344 sats -> 1,639,000 after the lot floor; house fills pay no fee.
    assert_eq!(
        tokens::balance(&state, bob.addr(), &btc()),
        10_000_000_000 + 1_639_000
    );
    let spent = 1_639_000u128 * px(61_000) / 100_000_000;
    assert_eq!(
        tokens::balance(&state, bob.addr(), &usds()),
        1_000_000_000_000 - spent
    );
    let escrow = AccountKey::new(bob.addr(), usds(), "order_escrow").unwrap();
    assert_eq!(state.ledger.balance(&escrow), 0, "unspent budget unlocked");
    let pool = AccountKey::new(Address::SYSTEM, usds(), "swap_pool").unwrap();
    assert_eq!(state.ledger.balance(&pool), spent as i128);
    // Quote expires at end of block 10.
    apply_block(&mut state, &ctx(11), &[]);
    assert!(state.markets.pairs["BTC-KUSD"].house.is_none());
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn snapshot_round_trip_preserves_hash() {
    let (mut state, mut alice, bob) = setup();
    apply_block(
        &mut state,
        &ctx(1),
        &[alice.act(Action::Transfer(Transfer {
            to: bob.addr(),
            asset: btc(),
            amount: 7,
            memo: Some("hi".into()),
        }))],
    );
    let restored = State::restore(&state.snapshot()).unwrap();
    assert_eq!(restored.compute_hash(), state.compute_hash());
    assert_eq!(restored.last_hash, state.last_hash);
}
