//! The pooled house-maker inventory: deposits mint shares on USD value,
//! house fills grow the pool, the epoch takes the protocol's share of the
//! gain in kind, withdrawals leave pro rata in kind after a delay, and the
//! quote limits and the reserve invariant see depositors' money.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{Action, HouseQuote, PlaceOrder, Proposal, ProposalKind, VoteChoice};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, Asset, OrderType, Side};
use keel_vm::{modules::liquidity, Event, VmError};

const KUSD: u128 = 1_000_000;
const KEEL: u128 = 1_000_000;
const SAT: u128 = 1;
const BTC: u128 = 100_000_000;

fn px(usd: u128) -> u128 {
    usd * KUSD
}

/// A real fill on `pair` so the pool can mark `base` at `price`: `maker`
/// rests `qty` at `price`, `taker` buys it.
fn set_price(c: &mut Chain, maker: &mut Actor, taker: &mut Actor, pair: &str, qty: u128, price: u128) {
    let (r, _) = c.block(&[maker.act(Action::PlaceOrder(PlaceOrder {
        pair: pair.into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(price),
        quantity: Some(qty),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    let (r, _) = c.block(&[taker.act(Action::PlaceOrder(PlaceOrder {
        pair: pair.into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(qty),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    assert_eq!(c.state.markets.pairs[pair].last_price, Some(price));
}

/// Put `amount` of a vault asset into the pool straight from reserves.
fn seed_pool_vault(state: &mut keel_vm::State, asset: &Asset, amount: u128, tag: &str) {
    state
        .ledger
        .post(
            &format!("seed:{tag}"),
            TxType::SystemFundsDeposit,
            None,
            None,
            vec![
                Record::debit(
                    AccountKey::new(Address::SYSTEM, asset.clone(), "vault_asset").unwrap(),
                    amount,
                ),
                Record::credit(
                    AccountKey::new(Address::SYSTEM, asset.clone(), "swap_pool").unwrap(),
                    amount,
                ),
            ],
        )
        .unwrap();
}

fn deposit(c: &mut Chain, who: &mut Actor, asset: Asset, amount: u128) -> keel_vm::Receipt {
    let (r, _) = c.block(&[who.act(Action::DepositLiquidity { asset, amount })]);
    r.into_iter().next().unwrap()
}

fn shares_of(c: &Chain, who: Address) -> u128 {
    c.state.liquidity.pools[liquidity::SWAP_POOL]
        .shares
        .get(&who)
        .copied()
        .unwrap_or(0)
}

fn quote(c: &mut Chain, op: &mut Actor, bid: Option<(u128, u128)>, ask: Option<(u128, u128)>) -> keel_vm::Receipt {
    let valid_until = c.height + 20;
    let (r, _) = c.block(&[op.act(Action::HouseQuote(HouseQuote {
        pair: "BTC-KUSD".into(),
        bid,
        ask,
        valid_until,
    }))]);
    r.into_iter().next().unwrap()
}

#[test]
fn first_deposit_prices_the_pool_and_mints_protocol_shares() {
    let (mut state, mut alice, mut bob, _v) = setup(0);
    fund_system(&mut state, &keel(), "swap_pool", 1_000 * KEEL, "genesis-keel");
    let mut c = Chain::new(state);

    // The pool holds KEEL without a price: nobody can be priced in fairly.
    let r = deposit(&mut c, &mut alice, usds(), 100 * KUSD);
    assert!(matches!(err(&r), VmError::Invalid(m) if m.contains("unpriced")));
    set_price(&mut c, &mut alice, &mut bob, "KEEL-KUSD", KEEL, KUSD);
    // Below the minimum.
    let r = deposit(&mut c, &mut alice, usds(), 5 * KUSD);
    assert!(matches!(err(&r), VmError::Invalid(_)));
    // An unpriced asset cannot be deposited either.
    let r = deposit(&mut c, &mut alice, btc(), BTC);
    assert!(matches!(err(&r), VmError::Invalid(m) if m.contains("no price")));

    let r = deposit(&mut c, &mut alice, usds(), 100 * KUSD);
    ok(&r);
    assert!(r.events.contains(&Event::LiquidityDeposited {
        owner: alice.addr(),
        asset: usds(),
        amount: 100 * KUSD,
        usd_value: 100 * KUSD,
        shares: 100 * KUSD,
    }));
    // The protocol owns the 1,000 KEEL (worth 1,000 USD) that was there.
    assert_eq!(shares_of(&c, Address::SYSTEM), 1_000 * KUSD);
    assert_eq!(shares_of(&c, alice.addr()), 100 * KUSD);
    let pool = &c.state.liquidity.pools[liquidity::SWAP_POOL];
    assert_eq!(pool.total_shares, 1_100 * KUSD);
    assert_eq!(pool.hwm, liquidity::VPS_SCALE);
    assert_eq!(sys(&c.state, &usds(), "swap_pool"), (100 * KUSD) as i128);
    // A second depositor at the same mark gets shares pro rata.
    let r = deposit(&mut c, &mut bob, usds(), 50 * KUSD);
    ok(&r);
    assert_eq!(shares_of(&c, bob.addr()), 50 * KUSD);
    assert_eq!(
        liquidity::pool_value_usd(&c.state),
        Some(1_150 * KUSD)
    );
    audit_clean(&c.state);
}

#[test]
fn house_fill_grows_the_pool_and_the_epoch_takes_the_protocol_share() {
    let (mut state, mut alice, mut bob, _v) = setup(0);
    state.params.epoch_length_blocks = 30;
    state.gov_house_operator = Some(alice.addr());
    seed_pool_vault(&mut state, &btc(), BTC, "btc");
    let mut c = Chain::new(state);
    set_price(&mut c, &mut alice, &mut bob, "BTC-KUSD", 1_000_000 * SAT, px(60_000));
    // A resting KEEL ask so the buyback can sweep the KUSD fee leg.
    let (r, _) = c.block(&[alice.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-KUSD".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(KUSD),
        quantity: Some(100_000 * KEEL),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    set_price(&mut c, &mut alice, &mut bob, "KEEL-KUSD", KEEL, KUSD);

    let r = deposit(&mut c, &mut bob, usds(), 1_000 * KUSD);
    ok(&r);
    // Pool: 1 BTC at 60,000 plus 1,000 KUSD; the protocol holds 60,000 shares.
    assert_eq!(shares_of(&c, Address::SYSTEM), 60_000 * KUSD);
    assert_eq!(shares_of(&c, bob.addr()), 1_000 * KUSD);

    // The house asks 61,000; bob lifts 0.1 BTC: the pool holds 0.9 BTC and
    // 7,100 KUSD, and the last fill marks BTC at 61,000.
    ok(&quote(&mut c, &mut alice, None, Some((px(61_000), BTC))));
    let (r, _) = c.block(&[bob.act(Action::PlaceOrder(PlaceOrder {
        pair: "BTC-KUSD".into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(10_000_000 * SAT),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    assert_eq!(sys(&c.state, &btc(), "swap_pool"), 90_000_000);
    assert_eq!(sys(&c.state, &usds(), "swap_pool"), (7_100 * KUSD) as i128);
    let value = liquidity::pool_value_usd(&c.state).unwrap();
    assert_eq!(value, 62_000 * KUSD);
    let hwm_value = 61_000 * KUSD;

    let events = c.advance_to(30);
    let fee_taken = events
        .iter()
        .find_map(|e| match e {
            Event::LiquidityFeeTaken { gain_usd, fee_usd, .. } => Some((*gain_usd, *fee_usd)),
            _ => None,
        })
        .expect("fee taken");
    assert_eq!(fee_taken.0, value - hwm_value);
    let half = (value - hwm_value) / 2;
    assert!(fee_taken.1 <= half && fee_taken.1 >= half - 2 * KUSD, "{fee_taken:?}");
    // In kind: both pool assets paid their share into the split.
    assert!(sys(&c.state, &btc(), "treasury") > 0);
    assert!(sys(&c.state, &btc(), "validator_rewards") > 0);
    assert!(sys(&c.state, &btc(), "burn") > 0);
    // The KUSD legs were swept into KEEL by the buyback in the same block,
    // after the pool's fee.
    let fee_at = events
        .iter()
        .position(|e| matches!(e, Event::LiquidityFeeTaken { .. }))
        .unwrap();
    let sweep_at = events
        .iter()
        .position(|e| matches!(e, Event::TreasurySwept { .. }))
        .expect("buyback swept the KUSD fee");
    assert!(fee_at < sweep_at);
    assert!(sys(&c.state, &keel(), "treasury") > 0);
    let pool = &c.state.liquidity.pools[liquidity::SWAP_POOL];
    assert!(pool.hwm > liquidity::VPS_SCALE);
    assert_eq!(pool.last_epoch.as_ref().unwrap().height, 30);
    let hwm_after = pool.hwm;

    // A loss epoch: BTC marks at 50,000; nothing is taken, the mark stays.
    set_price(&mut c, &mut alice, &mut bob, "BTC-KUSD", 1_000_000 * SAT, px(50_000));
    let events = c.advance_to(60);
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::LiquidityFeeTaken { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::LiquidityValued { height: 60, .. })));
    assert_eq!(c.state.liquidity.pools[liquidity::SWAP_POOL].hwm, hwm_after);
    audit_clean(&c.state);
}

#[test]
fn withdrawal_is_pro_rata_in_kind_after_the_delay_and_crystallises_gains() {
    let (mut state, mut alice, mut bob, _v) = setup(0);
    state.params.epoch_length_blocks = 1_000;
    state.liquidity.params.withdraw_delay_blocks = 3;
    state.gov_house_operator = Some(alice.addr());
    seed_pool_vault(&mut state, &btc(), BTC, "btc");
    let mut c = Chain::new(state);
    set_price(&mut c, &mut alice, &mut bob, "BTC-KUSD", 1_000_000 * SAT, px(60_000));
    ok(&deposit(&mut c, &mut bob, usds(), 6_000 * KUSD));
    // bob holds 6,000 of 66,000 shares: a tenth of the pool.
    let total = c.state.liquidity.pools[liquidity::SWAP_POOL].total_shares;
    assert_eq!(total, 66_000 * KUSD);

    // More than held is refused.
    let (r, _) = c.block(&[bob.act(Action::WithdrawLiquidity {
        shares: 7_000 * KUSD,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    // A gain first: the house sells 0.1 BTC at 61,000.
    ok(&quote(&mut c, &mut alice, None, Some((px(61_000), BTC))));
    let (r, _) = c.block(&[alice.act(Action::PlaceOrder(PlaceOrder {
        pair: "BTC-KUSD".into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(10_000_000 * SAT),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    let pool_btc = sys(&c.state, &btc(), "swap_pool") as u128;
    let pool_usd = sys(&c.state, &usds(), "swap_pool") as u128;
    let treasury_before = sys(&c.state, &usds(), "treasury");

    // bob leaves with half his shares: the protocol's share of the gain
    // is settled on the way out, then a twentieth of each asset moves to
    // his unlocking account.
    let (r, _) = c.block(&[bob.act(Action::WithdrawLiquidity {
        shares: 3_000 * KUSD,
    })]);
    ok(&r[0]);
    let release = c.height + 3;
    let fee_usd = r[0]
        .events
        .iter()
        .find_map(|e| match e {
            Event::LiquidityWithdrawQueued {
                release_height,
                fee_usd,
                ..
            } => {
                assert_eq!(*release_height, release);
                Some(*fee_usd)
            }
            _ => None,
        })
        .unwrap();
    assert!(fee_usd > 0, "unrealised gain crystallised");
    assert!(sys(&c.state, &usds(), "treasury") > treasury_before);
    let unlocking_btc = acct(&c.state, bob.addr(), &btc(), "liquidity_unlocking") as u128;
    let unlocking_usd = acct(&c.state, bob.addr(), &usds(), "liquidity_unlocking") as u128;
    assert!(unlocking_btc > 0 && unlocking_usd > 0);
    // 3,000 of 66,000 shares: a twenty-second of what the pool held after
    // the (small) fee legs.
    assert!(unlocking_btc <= pool_btc / 22 && unlocking_btc >= pool_btc / 22 - pool_btc / 400);
    assert!(unlocking_usd <= pool_usd / 22 && unlocking_usd >= pool_usd / 22 - pool_usd / 400);
    assert_eq!(shares_of(&c, bob.addr()), 3_000 * KUSD);
    let bob_btc_before = acct(&c.state, bob.addr(), &btc(), "deposit");

    // Not released before the delay, released at it.
    let events = c.advance_to(release - 1);
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::LiquidityUnlocked { .. })));
    assert_eq!(acct(&c.state, bob.addr(), &btc(), "deposit"), bob_btc_before);
    let events = c.advance_to(release);
    assert!(events.contains(&Event::LiquidityUnlocked {
        owner: bob.addr(),
        asset: btc(),
        amount: unlocking_btc,
    }));
    assert_eq!(
        acct(&c.state, bob.addr(), &btc(), "deposit"),
        bob_btc_before + unlocking_btc as i128
    );
    assert_eq!(acct(&c.state, bob.addr(), &btc(), "liquidity_unlocking"), 0);
    assert!(liquidity::pending_unlocks(&c.state, &bob.addr()).is_empty());
    audit_clean(&c.state);
}

#[test]
fn quote_limits_bound_the_inventory_and_the_spread() {
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    state.gov.param_admin = Some(v.addr());
    state.gov_house_operator = Some(alice.addr());
    seed_pool_vault(&mut state, &btc(), BTC, "btc");
    let mut c = Chain::new(state);
    let (r, _) = c.block(&[v.act(Action::SetParam {
        key: "liquidity.max_quote_inventory_bps".into(),
        value: 2_500,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[v.act(Action::SetParam {
        key: "liquidity.min_spread_bps".into(),
        value: 50,
    })]);
    ok(&r[0]);
    // 10 bps of spread is refused; a one-sided quote has no spread to check.
    let r = quote(&mut c, &mut alice, Some((px(60_940), BTC)), Some((px(61_000), BTC)));
    assert!(matches!(err(&r), VmError::Invalid(m) if m.contains("spread")));
    ok(&quote(&mut c, &mut alice, None, Some((px(61_000), BTC))));
    // The ask offers the whole BTC but only a quarter of the inventory is
    // on the table: a 0.5 BTC market buy fills 0.25.
    let bob_before = acct(&c.state, bob.addr(), &btc(), "deposit");
    let (r, _) = c.block(&[bob.act(Action::PlaceOrder(PlaceOrder {
        pair: "BTC-KUSD".into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(50_000_000 * SAT),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    assert_eq!(
        acct(&c.state, bob.addr(), &btc(), "deposit"),
        bob_before + 25_000_000
    );
    assert_eq!(sys(&c.state, &btc(), "swap_pool"), 75_000_000);
    audit_clean(&c.state);
}

#[test]
fn reserve_invariant_counts_depositors_share_of_pool_vault_assets() {
    let (state, mut alice, mut bob, _v) = setup(0);
    let mut c = Chain::new(state);
    set_price(&mut c, &mut alice, &mut bob, "BTC-KUSD", 1_000_000 * SAT, px(60_000));
    ok(&deposit(&mut c, &mut alice, btc(), 50_000_000 * SAT));
    // The pool was empty before alice: every pool satoshi is hers, and the
    // books still balance: reserves cover users' balances plus the pool.
    assert_eq!(
        liquidity::user_pool_liability(&c.state, c.height, &btc()),
        50_000_000
    );
    let events = c.advance_to(c.height + 1);
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::InvariantBreached { .. })));
    // Below activation the pool term is zero (the pre-0.2.0 reading).
    let mut inactive = c.state.clone();
    inactive.gov.upgrades.clear();
    assert_eq!(
        liquidity::user_pool_liability(&inactive, c.height, &btc()),
        0
    );
    // Reserves drop by 0.3 BTC with no liability leaving: only the pool
    // term makes that a breach.
    let drain = |s: &mut keel_vm::State| {
        s.ledger
            .post(
                "drain",
                TxType::SystemFundsDeposit,
                None,
                None,
                vec![
                    Record::debit(
                        AccountKey::new(Address::SYSTEM, btc(), "validator_rewards").unwrap(),
                        30_000_000,
                    ),
                    Record::credit(
                        AccountKey::new(Address::SYSTEM, btc(), "vault_asset").unwrap(),
                        30_000_000,
                    ),
                ],
            )
            .unwrap();
    };
    drain(&mut c.state);
    let events = c.advance_to(c.height + 1);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::InvariantBreached { asset, .. } if *asset == btc())));
    assert!(c.state.vaults.halted.contains(&btc()));
    // Without the pool term the drain would have hidden: 299.7 BTC of
    // reserves against 299.5 BTC of user balances.
    assert!(
        c.state.ledger.system_reserves(&btc()) as u128 > c.state.ledger.user_liabilities(&btc())
    );
}

#[test]
fn nothing_happens_below_the_activation_height() {
    let (mut state, mut alice, mut bob, _v) = setup(0);
    state.gov.upgrades.clear();
    state.gov.upgrades.push((liquidity::ACTIVATION_VERSION.into(), 6));
    state.liquidity.params.max_quote_inventory_bps = 1;
    state.gov_house_operator = Some(alice.addr());
    seed_pool_vault(&mut state, &btc(), BTC, "btc");
    let mut c = Chain::new(state);
    set_price(&mut c, &mut alice, &mut bob, "BTC-KUSD", 1_000_000 * SAT, px(60_000));
    // Height 3: refused; the clamp is the pre-activation whole balance.
    let r = deposit(&mut c, &mut alice, usds(), 100 * KUSD);
    assert!(matches!(err(&r), VmError::Invalid(m) if m.contains("not active")));
    ok(&quote(&mut c, &mut alice, None, Some((px(61_000), BTC))));
    let bob_before = acct(&c.state, bob.addr(), &btc(), "deposit");
    let (r, _) = c.block(&[bob.act(Action::PlaceOrder(PlaceOrder {
        pair: "BTC-KUSD".into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(10_000_000 * SAT),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    assert_eq!(
        acct(&c.state, bob.addr(), &btc(), "deposit"),
        bob_before + 10_000_000
    );
    assert_eq!(c.height, 5);
    // Height 6: active.
    let r = deposit(&mut c, &mut alice, usds(), 100 * KUSD);
    ok(&r);
    assert!(liquidity::active(&c.state, c.height));
    audit_clean(&c.state);
}

#[test]
fn liquidity_params_are_votable_and_bounded() {
    let (mut state, mut alice, _bob, mut v) = setup(100_000_000_000);
    state.params.proposal_deposit = 1_000_000_000;
    state.params.voting_period_blocks = 3;
    state.params.timelock_blocks = 1;
    state.gov.param_admin = Some(v.addr());
    let mut c = Chain::new(state);
    let id = c.state.gov.next_id;
    let (r, _) = c.block(&[alice.act(Action::Propose(Proposal {
        title: "share".into(),
        description: String::new(),
        kind: ProposalKind::ParamChange {
            key: "liquidity.protocol_share_bps".into(),
            value: 3_000,
        },
    }))]);
    ok(&r[0]);
    let p = c.state.gov.proposals[&id].clone();
    let (r, _) = c.block(&[v.act(Action::Vote {
        proposal_id: id,
        choice: VoteChoice::Yes,
    })]);
    ok(&r[0]);
    c.advance_to(p.timelock_end);
    let (r, _) = c.block(&[alice.act(Action::ExecuteProposal { proposal_id: id })]);
    ok(&r[0]);
    assert_eq!(c.state.liquidity.params.protocol_share_bps, 3_000);
    // Bounds through the admin path.
    let (r, _) = c.block(&[v.act(Action::SetParam {
        key: "liquidity.max_quote_inventory_bps".into(),
        value: 0,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    let (r, _) = c.block(&[v.act(Action::SetParam {
        key: "liquidity.protocol_share_bps".into(),
        value: 10_001,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    let (r, _) = c.block(&[v.act(Action::SetParam {
        key: "liquidity.withdraw_delay_blocks".into(),
        value: 100,
    })]);
    ok(&r[0]);
    assert_eq!(
        liquidity::get_param(&c.state, "liquidity.withdraw_delay_blocks"),
        Some(100)
    );
}

#[test]
fn the_pool_is_deterministic() {
    let run = || {
        let (mut state, mut alice, mut bob, _v) = setup(0);
        state.params.epoch_length_blocks = 12;
        state.liquidity.params.withdraw_delay_blocks = 2;
        state.gov_house_operator = Some(alice.addr());
        seed_pool_vault(&mut state, &btc(), BTC, "btc");
        let mut c = Chain::new(state);
        set_price(&mut c, &mut alice, &mut bob, "BTC-KUSD", 1_000_000 * SAT, px(60_000));
        ok(&deposit(&mut c, &mut bob, usds(), 2_000 * KUSD));
        ok(&quote(&mut c, &mut alice, Some((px(59_000), BTC)), Some((px(61_000), BTC))));
        let (r, _) = c.block(&[bob.act(Action::PlaceOrder(PlaceOrder {
            pair: "BTC-KUSD".into(),
            side: Side::Buy,
            order_type: OrderType::Market,
            price: None,
            quantity: Some(5_000_000 * SAT),
            quote_budget: None,
            client_id: None,
        }))]);
        ok(&r[0]);
        c.advance_to(12);
        let (r, _) = c.block(&[bob.act(Action::WithdrawLiquidity {
            shares: 1_000 * KUSD,
        })]);
        ok(&r[0]);
        c.advance_to(24);
        audit_clean(&c.state);
        c.state.compute_hash()
    };
    assert_eq!(run(), run());
}
