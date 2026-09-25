#![allow(clippy::unwrap_used)]
//! End-to-end VM tests for P2P offers, escrow trades and disputes.

use keel_actions::{Action, OfferSpec, Ruling, SignedAction, StartTrade, CHAIN_ID_DEVNET};
use keel_crypto::Keypair;
use keel_ledger::AccountKey;
use keel_types::{Address, Asset, Side};
use keel_vm::{
    apply_block,
    genesis::GenesisValidator,
    modules::{disputes, p2p::TradeStatus, tokens},
    BlockContext, Genesis, State, VmError,
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

const T0: u64 = 1_700_000_000;

fn setup() -> (State, Actor, Actor) {
    let alice = Actor::new(11);
    let bob = Actor::new(12);
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

/// Block `height` at `T0 + secs` seconds.
fn at(height: u64, secs: u64) -> BlockContext {
    BlockContext {
        height,
        timestamp: (T0 + secs) * 1000,
        proposer: None,
    }
}

fn btc() -> Asset {
    Asset::new("BTC.BTC")
}
fn keel() -> Asset {
    Asset::new("KEEL")
}
fn usds() -> Asset {
    Asset::new("KUSD")
}

fn sell_offer(asset: Asset, min: u128, max: u128) -> OfferSpec {
    OfferSpec {
        side: Side::Sell,
        asset,
        fiat_currency: "EUR".into(),
        payment_method: "SEPA".into(),
        margin_bps: 150,
        fixed_price: None,
        min_amount: min,
        max_amount: max,
        payment_window_secs: 1_800,
        country: Some("DE".into()),
        min_tier: 0,
        terms: "pay within 30 minutes".into(),
        instructions_hash: [7u8; 32],
    }
}

fn start(offer_id: u64, amount: u128) -> Action {
    Action::StartTrade(StartTrade {
        offer_id,
        amount,
        fiat_amount: 500_000,
        instructions_hash: [9u8; 32],
    })
}

fn sys(state: &State, asset: &Asset, t: &str) -> i128 {
    state
        .ledger
        .balance(&AccountKey::new(Address::SYSTEM, asset.clone(), t).unwrap())
}

fn escrow(state: &State, who: Address, asset: &Asset) -> i128 {
    state
        .ledger
        .balance(&AccountKey::new(who, asset.clone(), "marketplace_escrow").unwrap())
}

fn ok(r: &keel_vm::Receipt) {
    assert!(r.ok, "{:?}", r.error);
}

#[test]
fn create_offer_locks_deposit_and_close_refunds() {
    let (mut state, mut alice, _bob) = setup();
    let deposit = state.params.offer_deposit;
    let keel_before = tokens::balance(&state, alice.addr(), &keel());
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(sell_offer(
            btc(),
            1_000_000,
            100_000_000,
        )))],
    );
    ok(&r[0]);
    assert_eq!(
        tokens::balance(&state, alice.addr(), &keel()),
        keel_before - deposit
    );
    let dep = AccountKey::new(alice.addr(), keel(), "offer_deposit").unwrap();
    assert_eq!(state.ledger.balance(&dep), deposit as i128);
    assert_eq!(state.p2p.live_offers_of(&alice.addr()), 1);
    // Pause, update, close.
    let (r, _) = apply_block(
        &mut state,
        &at(2, 1),
        &[
            alice.act(Action::PauseOffer {
                offer_id: 0,
                paused: true,
            }),
            alice.act(Action::UpdateOffer {
                offer_id: 0,
                spec: sell_offer(btc(), 2_000_000, 50_000_000),
            }),
            alice.act(Action::CloseOffer { offer_id: 0 }),
        ],
    );
    r.iter().for_each(ok);
    assert_eq!(state.ledger.balance(&dep), 0);
    assert_eq!(tokens::balance(&state, alice.addr(), &keel()), keel_before);
    assert_eq!(state.p2p.live_offers_of(&alice.addr()), 0);
    assert!(state.p2p.offers[&0].closed);
    // A closed offer cannot be reopened.
    let (r, _) = apply_block(
        &mut state,
        &at(3, 2),
        &[alice.act(Action::PauseOffer {
            offer_id: 0,
            paused: false,
        })],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn offer_validation_and_fundability() {
    let (mut state, mut alice, _bob) = setup();
    // 200 BTC max on a 100 BTC balance: not fundable.
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(sell_offer(btc(), 1, 20_000_000_000)))],
    );
    assert_eq!(r[0].error, Some(VmError::NotEnoughFunds));
    let mut bad = sell_offer(btc(), 10, 5);
    let (r, _) = apply_block(
        &mut state,
        &at(2, 0),
        &[alice.act(Action::CreateOffer(bad.clone()))],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    bad = sell_offer(btc(), 1, 5);
    bad.payment_window_secs = 60;
    let (r, _) = apply_block(
        &mut state,
        &at(3, 0),
        &[alice.act(Action::CreateOffer(bad.clone()))],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    bad.payment_window_secs = 600;
    bad.margin_bps = 6_000;
    let (r, _) = apply_block(
        &mut state,
        &at(4, 0),
        &[alice.act(Action::CreateOffer(bad.clone()))],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    bad.margin_bps = 0;
    bad.asset = Asset::new("NOPE");
    let (r, _) = apply_block(
        &mut state,
        &at(5, 0),
        &[alice.act(Action::CreateOffer(bad))],
    );
    assert!(matches!(r[0].error, Some(VmError::NotFound(_))));
    assert!(state.p2p.offers.is_empty());
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn allowance_ladder_refuses_third_offer_for_a_fresh_account() {
    let (mut state, mut alice, _bob) = setup();
    assert_eq!(state.p2p.offer_allowance(&state.params, &alice.addr()), 2);
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[
            alice.act(Action::CreateOffer(sell_offer(
                btc(),
                1_000_000,
                10_000_000,
            ))),
            alice.act(Action::CreateOffer(sell_offer(
                btc(),
                1_000_000,
                10_000_000,
            ))),
            alice.act(Action::CreateOffer(sell_offer(
                btc(),
                1_000_000,
                10_000_000,
            ))),
        ],
    );
    ok(&r[0]);
    ok(&r[1]);
    assert!(
        matches!(r[2].error, Some(VmError::Invalid(_))),
        "{:?}",
        r[2].error
    );
    assert_eq!(state.p2p.live_offers_of(&alice.addr()), 2);
    // Ten completed trades unlock two more slots.
    state.p2p.completed_trades.insert(alice.addr(), 10);
    assert_eq!(state.p2p.offer_allowance(&state.params, &alice.addr()), 4);
    let (r, _) = apply_block(
        &mut state,
        &at(2, 0),
        &[alice.act(Action::CreateOffer(sell_offer(
            btc(),
            1_000_000,
            10_000_000,
        )))],
    );
    ok(&r[0]);
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn sell_offer_trade_happy_path_with_fee_split() {
    let (mut state, mut alice, mut bob) = setup();
    let amount = 100_000_000u128; // 1 BTC
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(sell_offer(btc(), 1_000_000, amount)))],
    );
    ok(&r[0]);
    let alice_btc = tokens::balance(&state, alice.addr(), &btc());
    let bob_btc = tokens::balance(&state, bob.addr(), &btc());

    // Bob (taker) buys: Alice's escrow holds amount + 1% fee (no USD price for BTC, so no surcharge).
    let (r, _) = apply_block(&mut state, &at(2, 10), &[bob.act(start(0, amount))]);
    ok(&r[0]);
    let fee = amount / 100;
    assert_eq!(escrow(&state, alice.addr(), &btc()), (amount + fee) as i128);
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        alice_btc - amount - fee
    );
    let t = &state.p2p.trades[&0];
    assert_eq!(
        (t.buyer, t.seller, t.status),
        (bob.addr(), alice.addr(), TradeStatus::Funded)
    );
    assert_eq!(t.deadline, T0 + 10 + 1_800);
    assert!(state.p2p.open_trades.contains_key(&0));

    // Only the buyer marks paid; only the seller releases.
    let (r, _) = apply_block(
        &mut state,
        &at(3, 20),
        &[alice.act(Action::MarkPaid {
            trade_id: 0,
            proof_hash: None,
        })],
    );
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
    let (r, _) = apply_block(
        &mut state,
        &at(4, 30),
        &[bob.act(Action::MarkPaid {
            trade_id: 0,
            proof_hash: Some([1u8; 32]),
        })],
    );
    ok(&r[0]);
    assert_eq!(state.p2p.trades[&0].status, TradeStatus::Paid);
    let (r, _) = apply_block(
        &mut state,
        &at(5, 40),
        &[bob.act(Action::ReleaseTrade { trade_id: 0 })],
    );
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
    let (r, _) = apply_block(
        &mut state,
        &at(6, 50),
        &[alice.act(Action::ReleaseTrade { trade_id: 0 })],
    );
    ok(&r[0]);

    assert_eq!(escrow(&state, alice.addr(), &btc()), 0);
    assert_eq!(
        tokens::balance(&state, bob.addr(), &btc()),
        bob_btc + amount
    );
    assert_eq!(sys(&state, &btc(), "treasury"), (fee / 2) as i128);
    assert_eq!(
        sys(&state, &btc(), "validator_rewards"),
        (fee * 4 / 10) as i128
    );
    assert_eq!(sys(&state, &btc(), "burn"), (fee / 10) as i128);
    assert_eq!(state.p2p.trades[&0].status, TradeStatus::Released);
    assert_eq!(state.p2p.completed_trades[&alice.addr()], 1);
    assert_eq!(state.p2p.completed_trades[&bob.addr()], 1);
    assert!(!state.p2p.open_trades.contains_key(&0));
    // Nothing further is possible on a released trade.
    let (r, _) = apply_block(
        &mut state,
        &at(7, 60),
        &[bob.act(Action::CancelTrade { trade_id: 0 })],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn small_stable_trade_pays_the_surcharge() {
    let (mut state, mut alice, mut bob) = setup();
    // 40 KUSD is below the $50 threshold: 1% + 1%.
    let amount = 40_000_000u128;
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(sell_offer(usds(), 1_000_000, amount)))],
    );
    ok(&r[0]);
    let (r, _) = apply_block(&mut state, &at(2, 1), &[bob.act(start(0, amount))]);
    ok(&r[0]);
    assert_eq!(state.p2p.trades[&0].fee, amount * 200 / 10_000);
    assert_eq!(
        escrow(&state, alice.addr(), &usds()),
        (amount + amount * 200 / 10_000) as i128
    );
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn start_trade_guards() {
    let (mut state, mut alice, mut bob) = setup();
    let mut spec = sell_offer(btc(), 1_000_000, 10_000_000);
    spec.fixed_price = Some(6_000_000); // EUR cents per whole BTC
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(spec))],
    );
    ok(&r[0]);
    // Self trade, out of bounds, wrong fiat at the fixed price, paused.
    let (r, _) = apply_block(
        &mut state,
        &at(2, 1),
        &[
            alice.act(start(0, 5_000_000)),
            bob.act(start(0, 50_000_000)),
            bob.act(Action::StartTrade(StartTrade {
                offer_id: 0,
                amount: 5_000_000,
                fiat_amount: 1,
                instructions_hash: [0u8; 32],
            })),
            bob.act(Action::StartTrade(StartTrade {
                offer_id: 0,
                amount: 5_000_000,
                fiat_amount: 300_000,
                instructions_hash: [0u8; 32],
            })),
        ],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    assert!(matches!(r[1].error, Some(VmError::Invalid(_))));
    assert!(matches!(r[2].error, Some(VmError::Invalid(_))));
    ok(&r[3]); // 0.05 BTC * 60,000.00 = 3,000.00 EUR
    let (r, _) = apply_block(
        &mut state,
        &at(3, 2),
        &[
            alice.act(Action::PauseOffer {
                offer_id: 0,
                paused: true,
            }),
            bob.act(start(0, 5_000_000)),
        ],
    );
    ok(&r[0]);
    assert!(matches!(r[1].error, Some(VmError::Invalid(_))));
    // Tier gate: a min_tier 2 offer refuses an unattested taker.
    let mut gated = sell_offer(btc(), 1_000_000, 10_000_000);
    gated.min_tier = 2;
    let (r, _) = apply_block(
        &mut state,
        &at(4, 3),
        &[
            alice.act(Action::CreateOffer(gated)),
            bob.act(start(1, 5_000_000)),
        ],
    );
    ok(&r[0]);
    assert_eq!(r[1].error, Some(VmError::Unauthorized));
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn buyer_cancel_before_paid_restores_escrow() {
    let (mut state, mut alice, mut bob) = setup();
    let amount = 10_000_000u128;
    let alice_btc = tokens::balance(&state, alice.addr(), &btc());
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(sell_offer(btc(), 1_000_000, amount)))],
    );
    ok(&r[0]);
    let (r, _) = apply_block(&mut state, &at(2, 1), &[bob.act(start(0, amount))]);
    ok(&r[0]);
    assert!(escrow(&state, alice.addr(), &btc()) > 0);
    let (r, _) = apply_block(
        &mut state,
        &at(3, 2),
        &[bob.act(Action::CancelTrade { trade_id: 0 })],
    );
    ok(&r[0]);
    assert_eq!(escrow(&state, alice.addr(), &btc()), 0);
    assert_eq!(tokens::balance(&state, alice.addr(), &btc()), alice_btc);
    assert_eq!(state.p2p.trades[&0].status, TradeStatus::Cancelled);
    assert!(!state.p2p.open_trades.contains_key(&0));
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn seller_cancels_only_after_the_payment_window() {
    let (mut state, mut alice, mut bob) = setup();
    let amount = 10_000_000u128;
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(sell_offer(btc(), 1_000_000, amount)))],
    );
    ok(&r[0]);
    let (r, _) = apply_block(&mut state, &at(2, 100), &[bob.act(start(0, amount))]);
    ok(&r[0]);
    assert!(!state.p2p.is_expired(0, T0 + 100));
    // Inside the window: refused. Exactly at the deadline: still refused.
    let (r, _) = apply_block(
        &mut state,
        &at(3, 200),
        &[alice.act(Action::CancelTrade { trade_id: 0 })],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    let (r, _) = apply_block(
        &mut state,
        &at(4, 1_900),
        &[alice.act(Action::CancelTrade { trade_id: 0 })],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    assert!(state.p2p.is_expired(0, T0 + 1_901));
    let (r, _) = apply_block(
        &mut state,
        &at(5, 1_901),
        &[alice.act(Action::CancelTrade { trade_id: 0 })],
    );
    ok(&r[0]);
    assert_eq!(escrow(&state, alice.addr(), &btc()), 0);
    // A stranger can never cancel.
    let (r, _) = apply_block(&mut state, &at(6, 2_000), &[bob.act(start(0, amount))]);
    ok(&r[0]);
    let mut carol = Actor::new(13);
    let (r, _) = apply_block(
        &mut state,
        &at(7, 2_001),
        &[carol.act(Action::CancelTrade { trade_id: 1 })],
    );
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
    // Once paid, nobody cancels.
    let (r, _) = apply_block(
        &mut state,
        &at(8, 2_002),
        &[bob.act(Action::MarkPaid {
            trade_id: 1,
            proof_hash: None,
        })],
    );
    ok(&r[0]);
    let (r, _) = apply_block(
        &mut state,
        &at(9, 9_000),
        &[
            alice.act(Action::CancelTrade { trade_id: 1 }),
            bob.act(Action::CancelTrade { trade_id: 1 }),
        ],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    assert!(matches!(r[1].error, Some(VmError::Invalid(_))));
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn dispute_opens_after_grace_and_non_arbitrators_cannot_rule() {
    let (mut state, mut alice, mut bob) = setup();
    let amount = 10_000_000u128;
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(sell_offer(btc(), 1_000_000, amount)))],
    );
    ok(&r[0]);
    let (r, _) = apply_block(&mut state, &at(2, 10), &[bob.act(start(0, amount))]);
    ok(&r[0]);
    // Unpaid trades cannot be disputed.
    let (r, _) = apply_block(
        &mut state,
        &at(3, 20),
        &[bob.act(Action::OpenDispute {
            trade_id: 0,
            evidence_hash: [1u8; 32],
        })],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    let (r, _) = apply_block(
        &mut state,
        &at(4, 30),
        &[bob.act(Action::MarkPaid {
            trade_id: 0,
            proof_hash: None,
        })],
    );
    ok(&r[0]);
    // Buyer must wait out the release grace; the seller may open at once.
    let (r, _) = apply_block(
        &mut state,
        &at(5, 40),
        &[bob.act(Action::OpenDispute {
            trade_id: 0,
            evidence_hash: [1u8; 32],
        })],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    let grace = state.params.release_grace_secs as u64;
    let (r, _) = apply_block(
        &mut state,
        &at(6, 30 + grace),
        &[bob.act(Action::OpenDispute {
            trade_id: 0,
            evidence_hash: [1u8; 32],
        })],
    );
    ok(&r[0]);
    assert_eq!(state.p2p.trades[&0].status, TradeStatus::Disputed);
    let escrowed = escrow(&state, alice.addr(), &btc());
    assert!(escrowed > 0);
    // Evidence from both parties, not from a stranger.
    let mut carol = Actor::new(13);
    let (r, _) = apply_block(
        &mut state,
        &at(7, 31 + grace),
        &[
            alice.act(Action::SubmitEvidence {
                trade_id: 0,
                evidence_hash: [2u8; 32],
            }),
            carol.act(Action::SubmitEvidence {
                trade_id: 0,
                evidence_hash: [3u8; 32],
            }),
        ],
    );
    ok(&r[0]);
    assert_eq!(r[1].error, Some(VmError::Unauthorized));
    assert_eq!(state.disputes.disputes[&0].evidence.len(), 2);
    // Nobody here is a bonded arbitrator: rulings are refused and escrow untouched.
    let (r, _) = apply_block(
        &mut state,
        &at(8, 32 + grace),
        &[
            alice.act(Action::RuleDispute {
                trade_id: 0,
                ruling: Ruling::WinsSeller,
            }),
            carol.act(Action::RuleDispute {
                trade_id: 0,
                ruling: Ruling::WinsBuyer,
            }),
        ],
    );
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
    assert_eq!(r[1].error, Some(VmError::Unauthorized));
    assert_eq!(escrow(&state, alice.addr(), &btc()), escrowed);
    assert!(state.disputes.disputes[&0].ruling.is_none());
    // Release and cancel are closed while disputed.
    let (r, _) = apply_block(
        &mut state,
        &at(9, 33 + grace),
        &[
            alice.act(Action::ReleaseTrade { trade_id: 0 }),
            bob.act(Action::CancelTrade { trade_id: 0 }),
        ],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    assert!(matches!(r[1].error, Some(VmError::Invalid(_))));
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn ruling_shares() {
    assert_eq!(disputes::shares(1_000, Ruling::WinsSeller), (0, 1_000));
    assert_eq!(disputes::shares(1_000, Ruling::WinsBuyer), (1_000, 0));
    assert_eq!(
        disputes::shares(1_000, Ruling::Split { buyer_bps: 2_500 }),
        (250, 750)
    );
    assert_eq!(
        disputes::shares(1_001, Ruling::Split { buyer_bps: 5_000 }),
        (500, 501)
    );
}

/// Happy ruling path: an elected, bonded arbitrator splits the escrow.
#[test]
fn arbitrator_split_ruling_pays_out_and_charges_dispute_fee() {
    let (mut state, mut alice, mut bob) = setup();
    let mut arb = Actor::new(14);
    // Membership is governance's decision; the bond is what the arbitrator
    // has to lose. Elect + fund + bond.
    keel_vm::modules::staking::set_arbitrators(&mut state, &[arb.addr()]);
    let min_bond = state.params.min_arbitrator_bond;
    state
        .ledger
        .post(
            "fund:arb",
            keel_ledger::TxType::SystemFundsDeposit,
            None,
            None,
            vec![
                keel_ledger::Record::debit(
                    AccountKey::new(Address::SYSTEM, keel(), "issuance").unwrap(),
                    min_bond,
                ),
                keel_ledger::Record::credit(
                    AccountKey::new(arb.addr(), keel(), "deposit").unwrap(),
                    min_bond,
                ),
            ],
        )
        .unwrap();
    let (r, _) = apply_block(
        &mut state,
        &at(0, 0),
        &[arb.act(Action::Bond(keel_actions::Bond {
            role: keel_actions::Role::Arbitrator,
            amount: min_bond,
            consensus_key: None,
        }))],
    );
    ok(&r[0]);
    assert!(keel_vm::modules::staking::is_arbitrator(
        &state,
        &arb.addr()
    ));
    let amount = 10_000_000u128;
    let (r, _) = apply_block(
        &mut state,
        &at(1, 0),
        &[alice.act(Action::CreateOffer(sell_offer(btc(), 1_000_000, amount)))],
    );
    ok(&r[0]);
    let (r, _) = apply_block(&mut state, &at(2, 10), &[bob.act(start(0, amount))]);
    ok(&r[0]);
    let (r, _) = apply_block(
        &mut state,
        &at(3, 20),
        &[bob.act(Action::MarkPaid {
            trade_id: 0,
            proof_hash: None,
        })],
    );
    ok(&r[0]);
    let (r, _) = apply_block(
        &mut state,
        &at(4, 30),
        &[alice.act(Action::OpenDispute {
            trade_id: 0,
            evidence_hash: [1u8; 32],
        })],
    );
    ok(&r[0]);
    let alice_btc = tokens::balance(&state, alice.addr(), &btc());
    let bob_btc = tokens::balance(&state, bob.addr(), &btc());
    let (r, _) = apply_block(
        &mut state,
        &at(5, 40),
        &[arb.act(Action::RuleDispute {
            trade_id: 0,
            ruling: Ruling::Split { buyer_bps: 7_000 },
        })],
    );
    ok(&r[0]);
    let fee = amount / 100;
    let dispute_fee = amount * state.params.dispute_fee_bps as u128 / 10_000;
    // Seller loses (30%): dispute fee out of the seller's 30%.
    assert_eq!(
        tokens::balance(&state, bob.addr(), &btc()),
        bob_btc + amount * 7 / 10
    );
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        alice_btc + amount * 3 / 10 - dispute_fee
    );
    assert_eq!(escrow(&state, alice.addr(), &btc()), 0);
    let kept = (fee + dispute_fee) as i128;
    assert_eq!(
        sys(&state, &btc(), "treasury")
            + sys(&state, &btc(), "validator_rewards")
            + sys(&state, &btc(), "burn"),
        kept
    );
    assert_eq!(state.p2p.trades[&0].status, TradeStatus::Ruled);
    assert!(state.ledger.audit().mismatches.is_empty());
}
