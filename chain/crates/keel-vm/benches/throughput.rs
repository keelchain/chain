//! Single-node VM throughput: signed limit orders applied through
//! `apply_block`, half resting, half crossing. Run with
//! `cargo run --release -p keel-vm --bin throughput` is not available for a
//! bench target, so this is a plain binary-style bench: `cargo bench -p
//! keel-vm --bench throughput` prints actions/s.

// A benchmark measures wall time and prints rates; it is not chain code.
#![allow(
    clippy::disallowed_methods,
    clippy::float_arithmetic,
    clippy::cast_precision_loss
)]

use keel_actions::{Action, PlaceOrder, SignedAction, CHAIN_ID_DEVNET};
use keel_crypto::Keypair;
use keel_types::{OrderType, Side};
use keel_vm::{apply_block, genesis::GenesisValidator, BlockContext, Genesis};
use std::time::Instant;

fn main() {
    let accounts: Vec<Keypair> = (1..=200u64).map(Keypair::from_seed).collect();
    let addrs: Vec<_> = accounts.iter().map(|k| k.address()).collect();
    let v = Keypair::from_seed(9_999);
    let g = Genesis::devnet(
        CHAIN_ID_DEVNET,
        &addrs,
        vec![GenesisValidator {
            address: v.address(),
            consensus_key: v.address().0,
            bond: 0,
        }],
    );
    let mut state = g.build();

    // Pre-sign: signing is the client's cost, not the chain's.
    let per_account = 500u64;
    let total = per_account as usize * accounts.len();
    let t0 = Instant::now();
    let mut actions = Vec::with_capacity(total);
    for round in 0..per_account {
        for (i, k) in accounts.iter().enumerate() {
            let sell = i % 2 == 0;
            let price = 60_000_000_000u128 + (round % 50) as u128 * 10_000;
            actions.push(SignedAction::sign(
                k,
                round,
                CHAIN_ID_DEVNET,
                Action::PlaceOrder(PlaceOrder {
                    pair: "BTC-KUSD".into(),
                    side: if sell { Side::Sell } else { Side::Buy },
                    order_type: OrderType::Limit,
                    price: Some(price),
                    quantity: Some(100_000),
                    quote_budget: None,
                    client_id: None,
                }),
            ));
        }
    }
    let sign_secs = t0.elapsed().as_secs_f64();

    let block_size = 5_000;
    let t1 = Instant::now();
    let mut ok = 0usize;
    let mut fills = 0usize;
    for (h, chunk) in actions.chunks(block_size).enumerate() {
        let ctx = BlockContext {
            height: h as u64 + 1,
            timestamp: 1_700_000_000_000 + h as u64 * 250,
            proposer: None,
        };
        let (receipts, _) = apply_block(&mut state, &ctx, chunk);
        ok += receipts.iter().filter(|r| r.ok).count();
        fills += receipts
            .iter()
            .flat_map(|r| r.events.iter())
            .filter(|e| matches!(e, keel_vm::Event::OrderFilled { .. }))
            .count();
    }
    let apply_secs = t1.elapsed().as_secs_f64();
    // Where the time goes: signature verification alone.
    let t2 = Instant::now();
    let verified = actions.iter().filter(|a| a.verify()).count();
    let verify_secs = t2.elapsed().as_secs_f64();
    println!(
        "sig verify alone: {:.0} actions/s ({verified} ok)",
        total as f64 / verify_secs
    );
    let hash_secs = {
        let t = Instant::now();
        let _ = state.compute_hash();
        t.elapsed().as_secs_f64()
    };
    println!("actions={total} ok={ok} fills={fills}");
    println!(
        "signing: {:.0} actions/s (client side)",
        total as f64 / sign_secs
    );
    println!(
        "apply (incl. sig verify + per-block state hash): {:.0} actions/s",
        total as f64 / apply_secs
    );
    println!(
        "state hash alone: {:.1} ms for {} orders in state",
        hash_secs * 1000.0,
        state.markets.orders.len()
    );
    assert!(state.ledger.audit().mismatches.is_empty());
}
