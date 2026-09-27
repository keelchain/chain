//! Emits borsh + signature vectors the TypeScript SDK re-encodes byte for
//! byte (sdk/ts/test/vectors.json). Run with `cargo test -p keel-actions`.
#![allow(clippy::unwrap_used)]

use keel_actions::{
    Action, Chain, DepositObservation, HouseQuote, OfferSpec, PlaceOrder, Proof, Proposal,
    ProposalKind, SignedAction, StartTrade, Transfer, Withdraw, CHAIN_ID_DEVNET,
};
use keel_crypto::Keypair;
use keel_types::{Address, Asset, OrderId, OrderType, Side};
use serde_json::json;

fn vectors() -> Vec<(&'static str, Action)> {
    vec![
        (
            "transfer",
            Action::Transfer(Transfer {
                to: Address::tagged(2),
                asset: Asset::new("KUSD"),
                amount: 5_000_000,
                memo: Some("hi".into()),
            }),
        ),
        (
            "place_limit",
            Action::PlaceOrder(PlaceOrder {
                pair: "BTC-KUSD".into(),
                side: Side::Sell,
                order_type: OrderType::Limit,
                price: Some(60_000_000_000),
                quantity: Some(100_000_000),
                quote_budget: None,
                client_id: Some(7),
            }),
        ),
        (
            "place_market_budget",
            Action::PlaceOrder(PlaceOrder {
                pair: "BTC-KUSD".into(),
                side: Side::Buy,
                order_type: OrderType::Market,
                price: None,
                quantity: None,
                quote_budget: Some(1_000_000_000),
                client_id: None,
            }),
        ),
        (
            "cancel",
            Action::CancelOrder {
                order_id: OrderId(42),
            },
        ),
        (
            "house_quote",
            Action::HouseQuote(HouseQuote {
                pair: "BTC-KUSD".into(),
                bid: Some((59_000_000_000, 1_000_000)),
                ask: None,
                valid_until: 99,
            }),
        ),
        ("buy_budget", Action::BuyBudget { actions: 1000 }),
        (
            "create_offer",
            Action::CreateOffer(OfferSpec {
                side: Side::Sell,
                asset: Asset::new("BTC.BTC"),
                fiat_currency: "EGP".into(),
                payment_method: "bank".into(),
                margin_bps: -150,
                fixed_price: None,
                min_amount: 100_000,
                max_amount: 10_000_000,
                payment_window_secs: 1800,
                country: Some("EG".into()),
                min_tier: 2,
                terms: "fast".into(),
                instructions_hash: [9u8; 32],
            }),
        ),
        (
            "start_trade",
            Action::StartTrade(StartTrade {
                offer_id: 3,
                amount: 200_000,
                fiat_amount: 1_234_567,
                instructions_hash: [1u8; 32],
            }),
        ),
        (
            "mark_paid",
            Action::MarkPaid {
                trade_id: 3,
                proof_hash: Some([2u8; 32]),
            },
        ),
        (
            "withdraw",
            Action::Withdraw(Withdraw {
                asset: Asset::new("ETH.USDT"),
                to: "0xabc".into(),
                amount: 10,
            }),
        ),
        (
            "observe_deposit",
            Action::ObserveDeposit(DepositObservation {
                chain: Chain::Tron,
                asset: Asset::new("TRON.USDT"),
                tx_hash: [3u8; 32],
                index: 1,
                deposit_index: 12,
                amount: 5_000_000,
                external_height: 100,
                tip_height: 130,
                proof: Proof::None,
            }),
        ),
        (
            "request_address",
            Action::RequestDepositAddress {
                chain: Chain::Bitcoin,
            },
        ),
        (
            "propose_param",
            Action::Propose(Proposal {
                title: "fee".into(),
                description: "".into(),
                kind: ProposalKind::ParamChange {
                    key: "taker_fee_bps".into(),
                    value: 25,
                },
            }),
        ),
        (
            "propose_btc_checkpoint",
            Action::Propose(Proposal {
                title: "btc".into(),
                description: "".into(),
                kind: ProposalKind::SetBtcCheckpoint(keel_actions::BtcCheckpoint {
                    network: 3,
                    height: 10,
                    header: vec![0xab; 80],
                    period_start_time: 1_700_000_000,
                }),
            }),
        ),
        (
            "propose_eth_checkpoint",
            Action::Propose(Proposal {
                title: "eth".into(),
                description: "".into(),
                kind: ProposalKind::SetEthCheckpoint(keel_actions::EthCheckpoint {
                    period: 900,
                    committee_root: [1; 32],
                    next_committee_root: Some([2; 32]),
                    genesis_validators_root: [3; 32],
                    fork_version: [4, 0, 0, 0],
                    committee_size: 512,
                }),
            }),
        ),
        (
            "propose_token_contract",
            Action::Propose(Proposal {
                title: "usdt".into(),
                description: "".into(),
                kind: ProposalKind::SetTokenContract {
                    asset: Asset::new("ETH.USDT"),
                    contract: vec![0xda; 20],
                },
            }),
        ),
        (
            "vote",
            Action::Vote {
                proposal_id: 1,
                choice: keel_actions::VoteChoice::Veto,
            },
        ),
        (
            "attest",
            Action::Attest {
                subject: Address::tagged(5),
                tier: 3,
                expires_at: 1_800_000_000,
            },
        ),
        (
            "set_param",
            Action::SetParam {
                key: "taker_fee_bps".into(),
                value: 15,
            },
        ),
        (
            "propose_param_admin",
            Action::Propose(Proposal {
                title: "revoke".into(),
                description: "".into(),
                kind: ProposalKind::SetParamAdmin { admin: None },
            }),
        ),
        ("lock_budget", Action::LockBudget { amount: 5_000_000 }),
        ("unlock_budget", Action::UnlockBudget { amount: 2_500_000 }),
        (
            "authorize_session_key",
            Action::AuthorizeSessionKey {
                key: Address::tagged(7),
                scope: 3,
                expires_at: 1_800_000_000,
            },
        ),
        (
            "revoke_session_key",
            Action::RevokeSessionKey {
                key: Address::tagged(7),
            },
        ),
        (
            "register_lightning_node",
            Action::RegisterLightningNode {
                node_id: vec![2u8; 33],
            },
        ),
        (
            "observe_lightning_deposit",
            Action::ObserveLightningDeposit(keel_actions::LightningDepositObservation {
                invoice: "lnbcrt1test".into(),
                preimage: [7; 32],
                amount_msat: 2_500_000,
            }),
        ),
        (
            "observe_lightning_payout",
            Action::ObserveLightningPayout {
                outbound_id: 9,
                preimage: Some([5; 32]),
                fee_paid_msat: 3_000,
                success: true,
            },
        ),
        (
            "fund_lightning_pool",
            Action::FundLightningPool {
                amount: 5_000_000,
                to: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".into(),
            },
        ),
        (
            "announce_lightning_sweep",
            Action::AnnounceLightningSweep {
                tx_hash: [0xaa; 32],
                amount: 400_000,
            },
        ),
        (
            "set_client_fee",
            Action::SetClientFee(keel_actions::ClientFee {
                p2p_bps: 85,
                taker_bps: 20,
                withdraw_bps: 10,
            }),
        ),
        (
            "register_custody_vault",
            Action::RegisterCustodyVault(keel_actions::CustodyVaultRegistration {
                chain: Chain::Bitcoin,
                epoch: 1,
                public_key: vec![2u8; 33],
                chain_code: Some([7u8; 32]),
                signer_url: "https://signer.example.com".into(),
            }),
        ),
        (
            "request_custody_address",
            Action::RequestCustodyAddress {
                chain: Chain::Tron,
                custodian: Keypair::from_seed(9).address(),
            },
        ),
        (
            "observe_custody_deposit",
            Action::ObserveCustodyDeposit {
                custodian: Keypair::from_seed(9).address(),
                observation: DepositObservation {
                    chain: Chain::Tron,
                    asset: Asset::vault("TRON", "USDT"),
                    tx_hash: [0x33; 32],
                    index: 0,
                    deposit_index: 2,
                    amount: 5_000_000,
                    external_height: 100,
                    tip_height: 130,
                    proof: Proof::None,
                },
            },
        ),
        (
            "withdraw_custody",
            Action::WithdrawCustody(Withdraw {
                asset: Asset::vault("TRON", "USDT"),
                to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
                amount: 1_000_000,
            }),
        ),
    ]
}

#[test]
fn emit_ts_vectors() {
    let key = Keypair::from_seed(1);
    let mut out = Vec::new();
    for (i, (name, action)) in vectors().into_iter().enumerate() {
        let sa = SignedAction::sign(&key, i as u64, CHAIN_ID_DEVNET, action.clone());
        assert!(sa.verify());
        out.push(json!({
            "name": name,
            "action_json": serde_json::to_value(&action).unwrap(),
            "action_borsh": hex::encode(borsh::to_vec(&action).unwrap()),
            "nonce": i,
            "chain_id": CHAIN_ID_DEVNET,
            "envelope_borsh": hex::encode(borsh::to_vec(&sa.envelope).unwrap()),
            "digest": hex::encode(sa.envelope.digest()),
            "signature": hex::encode(sa.signature),
            "tx_id": hex::encode(sa.id()),
        }));
    }
    let doc = json!({
        "secret": hex::encode(key.secret_bytes()),
        "address": key.address().to_hex(),
        "vectors": out,
    });
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../sdk/ts/test/vectors.json"
    );
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
}
