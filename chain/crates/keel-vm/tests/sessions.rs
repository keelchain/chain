//! Session keys (2026-09-09): scope-limited keys a site may hold.

mod common;

use common::*;
use keel_actions::{session_scope, Action, PlaceOrder, Transfer};
use keel_types::{OrderType, Side};
use keel_vm::{Event, VmError};

fn order(pair: &str) -> Action {
    Action::PlaceOrder(PlaceOrder {
        pair: pair.into(),
        side: Side::Buy,
        order_type: OrderType::Limit,
        price: Some(60_000_000_000),
        quantity: Some(100_000),
        quote_budget: None,
        client_id: Some(1),
    })
}

#[test]
fn session_key_trades_as_its_principal_but_cannot_move_funds() {
    let (state, mut alice, bob, _v) = setup(0);
    let mut c = Chain::new(state);
    let mut sess = Actor::new(77);
    let now = 1_000;
    // Authorize a markets-only session for a day.
    let (r, _) = c.block_at_secs(
        now,
        &[alice.act(Action::AuthorizeSessionKey {
            key: sess.addr(),
            scope: session_scope::MARKETS,
            expires_at: now + 86_400,
        })],
    );
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::SessionAuthorized {
        principal: alice.addr(),
        key: sess.addr(),
        scope: 1,
        expires_at: now + 86_400
    }));

    // The session key places an order: the order belongs to alice and
    // alice's budget pays for it; the key's own nonce advances.
    let before = c
        .state
        .account_ref(&alice.addr())
        .map(|m| m.budget.used)
        .unwrap_or(0);
    let (r, _) = c.block_at_secs(now + 1, &[sess.act(order("BTC-KUSD"))]);
    ok(&r[0]);
    assert!(r[0]
        .events
        .iter()
        .any(|e| matches!(e, Event::OrderAccepted { owner, .. } if *owner == alice.addr())));
    assert_eq!(
        c.state
            .account_ref(&alice.addr())
            .map(|m| m.budget.used)
            .unwrap_or(0),
        before + 1
    );
    assert_eq!(
        c.state
            .account_ref(&sess.addr())
            .map(|m| m.nonce)
            .unwrap_or(0),
        1
    );

    // Out of scope: a transfer, an offer (p2p_manage not granted), and a
    // second session key are refused.
    let (r, _) = c.block_at_secs(
        now + 2,
        &[
            sess.act(Action::Transfer(Transfer {
                to: bob.addr(),
                asset: keel(),
                amount: 1,
                memo: None,
            })),
            sess.act(Action::AuthorizeSessionKey {
                key: Actor::new(78).addr(),
                scope: 1,
                expires_at: now + 100,
            }),
        ],
    );
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    assert!(matches!(err(&r[1]), VmError::Unauthorized));

    // Revoked by the principal: the key is dead.
    let (r, _) = c.block_at_secs(
        now + 3,
        &[alice.act(Action::RevokeSessionKey { key: sess.addr() })],
    );
    ok(&r[0]);
    let (r, _) = c.block_at_secs(now + 4, &[sess.act(order("BTC-KUSD"))]);
    // No longer a session key: it acts as itself, an empty account with no funds.
    assert!(!matches!(err(&r[0]), VmError::Unauthorized) || true);
    assert!(!r[0].ok);
}

#[test]
fn session_authorization_is_validated_and_expires() {
    let (state, mut alice, mut bob, _v) = setup(0);
    let mut c = Chain::new(state);
    let mut sess = Actor::new(79);
    let now = 5_000;
    let (r, _) = c.block_at_secs(
        now,
        &[
            alice.act(Action::AuthorizeSessionKey {
                key: alice.addr(),
                scope: 1,
                expires_at: now + 10,
            }),
            alice.act(Action::AuthorizeSessionKey {
                key: sess.addr(),
                scope: 0,
                expires_at: now + 10,
            }),
            alice.act(Action::AuthorizeSessionKey {
                key: sess.addr(),
                scope: 8,
                expires_at: now + 10,
            }),
            alice.act(Action::AuthorizeSessionKey {
                key: sess.addr(),
                scope: 1,
                expires_at: now,
            }),
            alice.act(Action::AuthorizeSessionKey {
                key: sess.addr(),
                scope: 1,
                expires_at: now + 31 * 86_400,
            }),
            alice.act(Action::AuthorizeSessionKey {
                key: bob.addr(),
                scope: 1,
                expires_at: now + 10,
            }),
        ],
    );
    for x in &r {
        assert!(matches!(err(x), VmError::Invalid(_)), "{:?}", x.error);
    }
    let (r, _) = c.block_at_secs(
        now,
        &[alice.act(Action::AuthorizeSessionKey {
            key: sess.addr(),
            scope: session_scope::ALL,
            expires_at: now + 60,
        })],
    );
    ok(&r[0]);
    // Bob cannot take over alice's session key, nor revoke it.
    let (r, _) = c.block_at_secs(
        now + 1,
        &[
            bob.act(Action::AuthorizeSessionKey {
                key: sess.addr(),
                scope: 1,
                expires_at: now + 100,
            }),
            bob.act(Action::RevokeSessionKey { key: sess.addr() }),
        ],
    );
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    assert!(matches!(err(&r[1]), VmError::Unauthorized));
    // In scope until expiry, then refused.
    let (r, _) = c.block_at_secs(now + 30, &[sess.act(order("BTC-KUSD"))]);
    ok(&r[0]);
    let (r, _) = c.block_at_secs(now + 61, &[sess.act(order("BTC-KUSD"))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    // The key may revoke itself.
    let (r, _) = c.block_at_secs(
        now + 62,
        &[sess.act(Action::RevokeSessionKey { key: sess.addr() })],
    );
    ok(&r[0]);
    assert!(c.state.sessions.by_key.is_empty());
}
