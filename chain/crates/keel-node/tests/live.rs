//! A real `keel-node` process: submit an action over HTTP, read its receipt,
//! and fetch the snapshot the node serves for state sync.
#![allow(
    clippy::unwrap_used,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use keel_actions::{Action, SignedAction, Transfer, CHAIN_ID_DEVNET};
use keel_crypto::Keypair;
use keel_types::Asset;
use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Node {
    child: Child,
    rpc: String,
    _dir: tempdir::TempDir,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn port_is_free(p: u16) -> bool {
    TcpListener::bind(("127.0.0.1", p)).is_ok()
}

/// A p2p port whose metrics (+1000) and RPC (+2000) companions are free
/// too: tests start several nodes at once and the node binds all three.
fn free_port_triplet() -> u16 {
    for _ in 0..200 {
        let p = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        if p < 60_000 && port_is_free(p + 1000) && port_is_free(p + 2000) {
            return p;
        }
    }
    panic!("no free port triplet");
}

mod tempdir {
    pub struct TempDir(pub std::path::PathBuf);
    impl TempDir {
        pub fn new(tag: &str) -> Self {
            // Several nodes live in one test process: the tag (a port)
            // must make the directory unique, not the process id.
            let p = std::env::temp_dir().join(format!("keel-live-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// Tests start nodes concurrently; picking ports and binding them happens
/// under one lock so two nodes never race for the same triplet.
static START: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn start_node() -> Node {
    let _guard = START.lock().await;
    let p2p = free_port_triplet();
    let rpc_port = p2p + 2000;
    let dir = tempdir::TempDir::new(&format!("node-{p2p}"));
    let child = Command::new(env!("CARGO_BIN_EXE_keel-node"))
        .args([
            "--me",
            &format!("0@{p2p}"),
            "--participants",
            "0",
            "--devnet",
            "--devnet-idle-ms",
            "300",
            "--rpc-port",
            &rpc_port.to_string(),
            "--snapshot-interval",
            "5",
            "--storage-dir",
            dir.0.to_str().unwrap(),
            "--log-level",
            "info",
        ])
        .stdout(Stdio::null())
        .stderr(
            match std::fs::File::create(std::env::temp_dir().join(format!("keel-live-{p2p}.log"))) {
                Ok(f) => Stdio::from(f),
                Err(_) => Stdio::null(),
            },
        )
        .spawn()
        .expect("spawn keel-node");
    let node = Node {
        child,
        rpc: format!("http://127.0.0.1:{rpc_port}"),
        _dir: dir,
    };
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(r) = client.get(format!("{}/v1/status", node.rpc)).send().await {
            if r.status().is_success() {
                break;
            }
        }
        assert!(Instant::now() < deadline, "node RPC did not come up");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    node
}

async fn wait_height(client: &reqwest::Client, rpc: &str, target: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let v: serde_json::Value = client
            .get(format!("{rpc}/v1/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let h = v["height"].as_u64().unwrap();
        if h >= target {
            return h;
        }
        assert!(Instant::now() < deadline, "stuck at height {h}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn submit_receipt_and_state_sync_snapshot() {
    let node = start_node().await;
    let client = reqwest::Client::new();
    let rpc = node.rpc.clone();

    // The devnet funds the participant's account key (seed 0).
    let alice = Keypair::from_seed(0);
    let bob = Keypair::from_seed(1);
    let sa = SignedAction::sign(
        &alice,
        0,
        CHAIN_ID_DEVNET,
        Action::Transfer(Transfer {
            to: bob.address(),
            asset: Asset::new("KEEL"),
            amount: 1_000_000,
            memo: None,
        }),
    );
    let tx_id = hex::encode(sa.id());
    let r = client
        .post(format!("{rpc}/v1/actions"))
        .json(&sa)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 202, "{}", r.text().await.unwrap());

    let deadline = Instant::now() + Duration::from_secs(60);
    let receipt: serde_json::Value = loop {
        let r = client
            .get(format!("{rpc}/v1/receipts/{tx_id}"))
            .send()
            .await
            .unwrap();
        if r.status().is_success() {
            break r.json().await.unwrap();
        }
        assert!(Instant::now() < deadline, "no receipt for {tx_id}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(receipt["ok"], serde_json::Value::Bool(true), "{receipt}");

    // An unknown id explains where older receipts live.
    let r = client
        .get(format!("{rpc}/v1/receipts/{}", "00".repeat(32)))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 404);
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("indexer"));

    // The balance moved.
    let acct: serde_json::Value = client
        .get(format!("{rpc}/v1/accounts/{}", bob.address().to_hex()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let keel = acct["balances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["asset"] == "KEEL" && b["account_type"] == "deposit")
        .map(|b| b["balance"].as_str().unwrap().parse::<u128>().unwrap())
        .unwrap_or(0);
    assert!(keel >= 1_000_000, "bob holds {keel}");

    // After the first snapshot the node serves it for state sync, and the
    // bytes decode to the state it describes.
    wait_height(&client, &rpc, 6).await;
    let meta: keel_rpc::SyncMeta = client
        .get(format!("{rpc}/v1/sync/meta"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        meta.height >= 5 && meta.height.is_multiple_of(5),
        "{meta:?}"
    );
    let bytes = client
        .get(format!("{rpc}/v1/sync/snapshot"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let state = keel_vm::State::restore(&bytes).expect("snapshot decodes");
    assert_eq!(state.height, meta.height);
    assert_eq!(hex::encode(state.compute_hash()), meta.state_hash);
    assert_eq!(hex::encode(state.last_hash), meta.last_hash);
    let status: serde_json::Value = client
        .get(format!("{rpc}/v1/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["oldest_block"], serde_json::json!(1));

    // Readiness: the devnet has no vault yet, so Bitcoin is not ready and
    // the reasons say why.
    let ready: serde_json::Value = client
        .get(format!("{rpc}/v1/ready/BTC"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ready["chain"], "BTC");
    assert_eq!(ready["ready"], serde_json::Value::Bool(false));
    let reasons = ready["reasons"].as_array().unwrap();
    assert!(
        reasons
            .iter()
            .any(|r| r.as_str().unwrap().contains("no vault")),
        "{ready}"
    );
    assert_eq!(ready["outbound_pending"], 0);
}

/// The filtered WebSocket: subscribe to an account and a book, submit a
/// transfer, and receive the receipt on the account channel with monotonic
/// sequence numbers.
#[tokio::test]
async fn websocket_account_channel_delivers_receipts() {
    use futures_util::{SinkExt as _, Stream, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;

    let node = start_node().await;
    let client = reqwest::Client::new();
    let rpc = node.rpc.clone();
    let ws_url = format!("{}/v1/ws", rpc.replace("http://", "ws://"));
    let (mut socket, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("ws connect");

    let alice = Keypair::from_seed(0);
    let bob = Keypair::from_seed(1);
    let sub = serde_json::json!({
        "op": "subscribe",
        "channels": [format!("account:{}", bob.address().to_hex()), "book:BTC-KUSD?depth=5"],
    });
    socket
        .send(Message::Text(sub.to_string().into()))
        .await
        .unwrap();

    async fn next_json(
        socket: &mut (impl Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
                  + Unpin),
    ) -> serde_json::Value {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(30), socket.next())
                .await
                .expect("ws message")
                .expect("ws open")
                .expect("ws frame");
            if let Message::Text(t) = m {
                return serde_json::from_str(&t).unwrap();
            }
        }
    }

    let first = next_json(&mut socket).await;
    assert_eq!(first["type"], "subscribed", "{first}");
    assert_eq!(first["seq"], 1);
    assert_eq!(first["channels"].as_array().unwrap().len(), 2);
    let snap = next_json(&mut socket).await;
    assert_eq!(snap["type"], "book_snapshot", "{snap}");
    assert_eq!(snap["pair"], "BTC-KUSD");
    assert_eq!(snap["seq"], 2);

    let sa = SignedAction::sign(
        &alice,
        0,
        CHAIN_ID_DEVNET,
        Action::Transfer(Transfer {
            to: bob.address(),
            asset: Asset::new("KEEL"),
            amount: 7,
            memo: None,
        }),
    );
    let tx_id = hex::encode(sa.id());
    let r = client
        .post(format!("{rpc}/v1/actions"))
        .json(&sa)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 202);

    // Only bob's receipt arrives on the account channel; heartbeats and
    // book messages may interleave.
    let mut last_seq = 2;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        assert!(
            Instant::now() < deadline,
            "no receipt on the account channel"
        );
        let m = next_json(&mut socket).await;
        let seq = m["seq"].as_u64().unwrap();
        assert!(seq > last_seq, "sequence went backwards: {m}");
        last_seq = seq;
        if m["type"] == "event" {
            assert_eq!(m["channel"], format!("account:{}", bob.address().to_hex()));
            assert_eq!(m["data"]["kind"], "receipt");
            assert_eq!(m["data"]["receipt"]["tx_id"], tx_id);
            break;
        }
        assert!(
            matches!(
                m["type"].as_str(),
                Some("heartbeat") | Some("book_delta") | Some("book_snapshot")
            ),
            "unexpected frame {m}"
        );
    }
    let _ = socket.close(None).await;
}
