#![allow(clippy::unwrap_used)]
//! Per-offer arbitration: the offer names bonded arbitrators, the taker
//! accepts them by taking the offer, one of them rules; offers that name
//! nobody fall back to the governance-elected set; governance can ban.

use keel_actions::{
    Action, Bond, OfferSpec, Role, Ruling, SignedAction, StartTrade, CHAIN_ID_DEVNET,
};
use keel_crypto::Keypair;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, Asset, Side};
use keel_vm::{
    apply_block,
    genesis::GenesisValidator,
    modules::{arbitration, p2p::TradeStatus, staking},
    BlockContext, Event, Genesis, State, VmError,
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

/// alice makes offers, bob takes them; arb1/arb2 hold bonds, elected is a
/// member of the fallback set, carol holds nothing.
fn setup() -> (State, Vec<Actor>) {
    let actors: Vec<Actor> = [11u64, 12, 13, 14, 15, 16]
        .into_iter()
        .map(Actor::new)
        .collect();
    let funded: Vec<Address> = actors.iter().map(Actor::addr).collect();
    let validator = Keypair::from_seed(100);
    let g = Genesis::devnet(
        CHAIN_ID_DEVNET,
        &funded,
        vec![GenesisValidator {
            address: validator.address(),
            consensus_key: validator.address().0,
            bond: 0,
        }],
    );
    (g.build(), actors)
}

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

fn sell_offer(max: u128) -> OfferSpec {
    OfferSpec {
        side: Side::Sell,
        asset: btc(),
        fiat_currency: "EUR".into(),
        payment_method: "SEPA".into(),
        margin_bps: 150,
        fixed_price: None,
        min_amount: 1_000_000,
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

fn ok(r: &keel_vm::Receipt) {
    assert!(r.ok, "{:?}", r.error);
}

/// Bond the minimum as an arbitrator (funded from issuance).
fn bond_arbitrator(state: &mut State, who: &mut Actor, height: u64) {
    let min_bond = state.params.min_arbitrator_bond;
    state
        .ledger
        .post(
            &format!("fund:arb:{}", who.addr().to_hex()),
            TxType::SystemFundsDeposit,
            None,
            None,
            vec![
                Record::debit(
                    AccountKey::new(Address::SYSTEM, Asset::new("KEEL"), "issuance").unwrap(),
                    min_bond,
                ),
                Record::credit(
                    AccountKey::new(who.addr(), Asset::new("KEEL"), "deposit").unwrap(),
                    min_bond,
                ),
            ],
        )
        .unwrap();
    let (r, _) = apply_block(
        state,
        &at(height, 0),
        &[who.act(Action::Bond(Bond {
            role: Role::Arbitrator,
            amount: min_bond,
            consensus_key: None,
        }))],
    );
    ok(&r[0]);
    assert!(staking::has_arbitrator_bond(state, &who.addr()));
}

fn bound_to(r: &keel_vm::Receipt) -> Option<Address> {
    r.events.iter().find_map(|e| match e {
        Event::TradeArbitratorBound { arbitrator, .. } => Some(*arbitrator),
        _ => None,
    })
}

/// Take `offer`, pay, wait out the grace, open a dispute. Returns the trade id.
fn trade_to_dispute(state: &mut State, bob: &mut Actor, offer: u64, h: &mut u64) -> (u64, Address) {
    *h += 1;
    let (r, _) = apply_block(state, &at(*h, 0), &[bob.act(start(offer, 5_000_000))]);
    ok(&r[0]);
    let trade_id = r[0]
        .events
        .iter()
        .find_map(|e| match e {
            Event::TradeStarted { trade_id, .. } => Some(*trade_id),
            _ => None,
        })
        .unwrap();
    let bound = bound_to(&r[0]);
    *h += 1;
    let (r, _) = apply_block(
        state,
        &at(*h, 10),
        &[bob.act(Action::MarkPaid {
            trade_id,
            proof_hash: None,
        })],
    );
    ok(&r[0]);
    let bound = bound.or(bound_to(&r[0]));
    *h += 1;
    let (r, _) = apply_block(
        state,
        &at(*h, 4_000),
        &[bob.act(Action::OpenDispute {
            trade_id,
            evidence_hash: [1u8; 32],
        })],
    );
    ok(&r[0]);
    (trade_id, bound.unwrap_or(Address::SYSTEM))
}

fn rule(state: &mut State, who: &mut Actor, trade_id: u64, h: &mut u64) -> keel_vm::Receipt {
    *h += 1;
    let (r, _) = apply_block(
        state,
        &at(*h, 5_000),
        &[who.act(Action::RuleDispute {
            trade_id,
            ruling: Ruling::WinsBuyer,
        })],
    );
    r.into_iter().next().unwrap()
}

#[test]
fn named_arbitrator_rules_and_the_fallback_set_is_refused() {
    let (mut state, mut a) = setup();
    let [alice, bob, arb1, _arb2, elected, carol] = &mut a[..] else {
        unreachable!()
    };
    let mut h = 1;
    bond_arbitrator(&mut state, arb1, h);
    staking::set_arbitrators(&mut state, &[elected.addr()]);
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[alice.act(Action::CreateOffer(sell_offer(10_000_000)))],
    );
    ok(&r[0]);

    // Only the owner names; only bonded, unbanned addresses can be named.
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[
            bob.act(Action::SetOfferArbitrators {
                offer_id: 0,
                arbitrators: vec![arb1.addr()],
            }),
            alice.act(Action::SetOfferArbitrators {
                offer_id: 0,
                arbitrators: vec![carol.addr()],
            }),
            alice.act(Action::SetOfferArbitrators {
                offer_id: 0,
                arbitrators: vec![elected.addr()],
            }),
            alice.act(Action::SetOfferArbitrators {
                offer_id: 0,
                arbitrators: vec![arb1.addr(), arb1.addr()],
            }),
            alice.act(Action::SetOfferArbitrators {
                offer_id: 0,
                arbitrators: vec![arb1.addr()],
            }),
        ],
    );
    assert!(matches!(r[0].error, Some(VmError::Unauthorized)));
    assert!(matches!(r[1].error, Some(VmError::Invalid(_))), "no bond");
    assert!(
        matches!(r[2].error, Some(VmError::Invalid(_))),
        "elected without a bond is not eligible to be named"
    );
    assert!(matches!(r[3].error, Some(VmError::Invalid(_))), "duplicate");
    ok(&r[4]);
    assert!(r[4].events.iter().any(|e| matches!(
        e,
        Event::OfferArbitratorsSet {
            offer_id: 0,
            count: 1
        }
    )));

    // One name binds at start; the fallback member may not rule it.
    let (trade_id, bound) = trade_to_dispute(&mut state, bob, 0, &mut h);
    assert_eq!(bound, arb1.addr());
    assert_eq!(
        state.arbitration.trade_arbitrator.get(&trade_id),
        Some(&arb1.addr())
    );
    let r = rule(&mut state, elected, trade_id, &mut h);
    assert!(matches!(r.error, Some(VmError::Unauthorized)));
    let r = rule(&mut state, carol, trade_id, &mut h);
    assert!(matches!(r.error, Some(VmError::Unauthorized)));
    let r = rule(&mut state, arb1, trade_id, &mut h);
    ok(&r);
    assert_eq!(state.p2p.trades[&trade_id].status, TradeStatus::Ruled);

    // Clearing the list: later trades fall back to the elected set.
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[alice.act(Action::SetOfferArbitrators {
            offer_id: 0,
            arbitrators: vec![],
        })],
    );
    ok(&r[0]);
    assert!(!state.arbitration.offer_arbitrators.contains_key(&0));
    let (trade_id, bound) = trade_to_dispute(&mut state, bob, 0, &mut h);
    assert_eq!(bound, Address::SYSTEM, "nothing bound");
    let r = rule(&mut state, arb1, trade_id, &mut h);
    assert!(
        matches!(r.error, Some(VmError::Unauthorized)),
        "bonded but not elected"
    );
    let r = rule(&mut state, elected, trade_id, &mut h);
    ok(&r);
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn taker_chooses_among_several_or_the_first_applies_at_mark_paid() {
    let (mut state, mut a) = setup();
    let [alice, bob, arb1, arb2, elected, carol] = &mut a[..] else {
        unreachable!()
    };
    let mut h = 1;
    bond_arbitrator(&mut state, arb1, h);
    bond_arbitrator(&mut state, arb2, h);
    staking::set_arbitrators(&mut state, &[elected.addr()]);
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[alice.act(Action::CreateOffer(sell_offer(10_000_000)))],
    );
    ok(&r[0]);
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[alice.act(Action::SetOfferArbitrators {
            offer_id: 0,
            arbitrators: vec![arb1.addr(), arb2.addr()],
        })],
    );
    ok(&r[0]);

    // Several names: nothing binds at start; the taker picks one.
    h += 1;
    let (r, _) = apply_block(&mut state, &at(h, 0), &[bob.act(start(0, 5_000_000))]);
    ok(&r[0]);
    assert!(bound_to(&r[0]).is_none());
    assert_eq!(
        state.arbitration.trade_candidates.get(&0),
        Some(&vec![arb1.addr(), arb2.addr()])
    );
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[
            carol.act(Action::ChooseArbitrator {
                trade_id: 0,
                arbitrator: arb2.addr(),
            }),
            alice.act(Action::ChooseArbitrator {
                trade_id: 0,
                arbitrator: arb2.addr(),
            }),
            bob.act(Action::ChooseArbitrator {
                trade_id: 0,
                arbitrator: elected.addr(),
            }),
            bob.act(Action::ChooseArbitrator {
                trade_id: 0,
                arbitrator: arb2.addr(),
            }),
            bob.act(Action::ChooseArbitrator {
                trade_id: 0,
                arbitrator: arb1.addr(),
            }),
        ],
    );
    assert!(
        matches!(r[0].error, Some(VmError::Unauthorized)),
        "stranger"
    );
    assert!(
        matches!(r[1].error, Some(VmError::Unauthorized)),
        "maker is not the taker"
    );
    assert!(matches!(r[2].error, Some(VmError::Invalid(_))), "not named");
    ok(&r[3]);
    assert_eq!(bound_to(&r[3]), Some(arb2.addr()));
    assert!(
        matches!(r[4].error, Some(VmError::Invalid(_))),
        "already bound"
    );
    // Choosing after MarkPaid is too late.
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 10),
        &[bob.act(Action::MarkPaid {
            trade_id: 0,
            proof_hash: None,
        })],
    );
    ok(&r[0]);
    assert!(bound_to(&r[0]).is_none(), "already chosen, nothing rebinds");

    // Second trade, no choice: the first named applies when paid.
    h += 1;
    let (r, _) = apply_block(&mut state, &at(h, 20), &[bob.act(start(0, 5_000_000))]);
    ok(&r[0]);
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 30),
        &[bob.act(Action::MarkPaid {
            trade_id: 1,
            proof_hash: None,
        })],
    );
    ok(&r[0]);
    assert_eq!(bound_to(&r[0]), Some(arb1.addr()));
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 40),
        &[bob.act(Action::ChooseArbitrator {
            trade_id: 1,
            arbitrator: arb2.addr(),
        })],
    );
    assert!(
        matches!(r[0].error, Some(VmError::Invalid(_))),
        "paid trades are fixed"
    );

    // The param admin caps how many an offer may name.
    state.gov.param_admin = Some(alice.addr());
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 50),
        &[
            alice.act(Action::SetParam {
                key: "arbitration.max_named".into(),
                value: 1,
            }),
            alice.act(Action::SetOfferArbitrators {
                offer_id: 0,
                arbitrators: vec![arb1.addr(), arb2.addr()],
            }),
            alice.act(Action::SetParam {
                key: "arbitration.max_named".into(),
                value: 0,
            }),
        ],
    );
    ok(&r[0]);
    assert!(
        matches!(r[1].error, Some(VmError::Invalid(_))),
        "over the cap"
    );
    assert!(
        matches!(r[2].error, Some(VmError::Invalid(_))),
        "cap must be >= 1"
    );
    assert_eq!(
        arbitration::get_param(&state, "arbitration.max_named"),
        Some(1)
    );
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn a_banned_or_unbonded_arbitrator_loses_the_trade_to_the_fallback_set() {
    let (mut state, mut a) = setup();
    let [alice, bob, arb1, _arb2, elected, _carol] = &mut a[..] else {
        unreachable!()
    };
    let mut h = 1;
    bond_arbitrator(&mut state, arb1, h);
    staking::set_arbitrators(&mut state, &[elected.addr()]);
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[alice.act(Action::CreateOffer(sell_offer(10_000_000)))],
    );
    ok(&r[0]);
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[alice.act(Action::SetOfferArbitrators {
            offer_id: 0,
            arbitrators: vec![arb1.addr()],
        })],
    );
    ok(&r[0]);
    let (trade_id, bound) = trade_to_dispute(&mut state, bob, 0, &mut h);
    assert_eq!(bound, arb1.addr());

    // Governance bans arb1 (what the BanArbitrator proposal executes): the
    // bound trade falls back to the elected set, and arb1 can no longer be
    // named anywhere.
    let events = arbitration::ban(&mut state, arb1.addr());
    assert!(matches!(events[0], Event::ArbitratorBanned { .. }));
    let r = rule(&mut state, arb1, trade_id, &mut h);
    assert!(matches!(r.error, Some(VmError::Unauthorized)));
    h += 1;
    let (r, _) = apply_block(
        &mut state,
        &at(h, 0),
        &[alice.act(Action::SetOfferArbitrators {
            offer_id: 0,
            arbitrators: vec![arb1.addr()],
        })],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(_))));
    let r = rule(&mut state, elected, trade_id, &mut h);
    ok(&r);
    assert_eq!(state.p2p.trades[&trade_id].status, TradeStatus::Ruled);
    // A banned address in the elected set cannot rule either.
    arbitration::ban(&mut state, elected.addr());
    assert!(!arbitration::may_rule(&state, trade_id, &elected.addr()));
    assert!(state.ledger.audit().mismatches.is_empty());
}
