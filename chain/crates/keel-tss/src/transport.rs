//! Round-message transport between the `n` parties of a signing set.
//!
//! The protocol layer ([`crate::protocol`]) only needs two primitives:
//! send one message (to one party or to everyone else) and receive the
//! next message. Every message is tagged with a `session` string so
//! consecutive protocols (keygen, then aux-info generation, then many
//! signing sessions) can share one link without their messages mixing.
//!
//! Two implementations ship:
//! - [`InMemoryNetwork`]: channels between threads, for tests.
//! - [`TcpTransport`]: one JSON line per message over plain TCP, every
//!   line authenticated with HMAC-SHA256 over a secret shared by the set.
//!   Authentication proves membership of the set, not the identity of the
//!   sender (all parties hold the same secret); run it over a private
//!   network or a TLS tunnel between the signer hosts.

use hmac::{Hmac, Mac as _};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufRead as _, BufReader, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

/// One message on the wire. `to == None` delivers to every other party of
/// the set; `broadcast` is the protocol-level message type (a broadcast
/// of a signing subset is still delivered party by party).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMessage {
    pub session: String,
    pub from: u16,
    pub to: Option<u16>,
    pub broadcast: bool,
    pub body: Vec<u8>,
}

impl WireMessage {
    pub fn broadcast(session: &str, from: u16, body: Vec<u8>) -> Self {
        Self {
            session: session.to_string(),
            from,
            to: None,
            broadcast: true,
            body,
        }
    }

    pub fn p2p(session: &str, from: u16, to: u16, body: Vec<u8>) -> Self {
        Self {
            session: session.to_string(),
            from,
            to: Some(to),
            broadcast: false,
            body,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("timed out waiting for a message")]
    Timeout,
    #[error("transport closed")]
    Closed,
    #[error("unknown party {0}")]
    UnknownParty(u16),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad frame: {0}")]
    BadFrame(String),
}

/// Synchronous, thread-safe message transport. The protocol runner calls
/// `send` and `recv` from one thread per protocol execution.
pub trait Transport: Send + Sync {
    fn my_index(&self) -> u16;
    fn n(&self) -> u16;
    fn send(&self, msg: &WireMessage) -> Result<(), TransportError>;
    /// Next message from any party, waiting at most `timeout`.
    fn recv(&self, timeout: Duration) -> Result<WireMessage, TransportError>;
}

// ---------------------------------------------------------------- in-memory

/// A full mesh of in-process parties.
pub struct InMemoryNetwork;

impl InMemoryNetwork {
    /// Build `n` connected transports; index `i` of the returned vector is
    /// party `i`.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(n: u16) -> Vec<InMemoryTransport> {
        let mut senders = Vec::with_capacity(n as usize);
        let mut receivers = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let (tx, rx) = mpsc::channel();
            senders.push(tx);
            receivers.push(rx);
        }
        receivers
            .into_iter()
            .enumerate()
            .map(|(i, rx)| InMemoryTransport {
                my_index: i as u16,
                n,
                senders: senders.clone(),
                rx: Mutex::new(rx),
            })
            .collect()
    }
}

pub struct InMemoryTransport {
    my_index: u16,
    n: u16,
    senders: Vec<mpsc::Sender<WireMessage>>,
    rx: Mutex<mpsc::Receiver<WireMessage>>,
}

impl Transport for InMemoryTransport {
    fn my_index(&self) -> u16 {
        self.my_index
    }

    fn n(&self) -> u16 {
        self.n
    }

    fn send(&self, msg: &WireMessage) -> Result<(), TransportError> {
        match msg.to {
            Some(to) => {
                let tx = self
                    .senders
                    .get(to as usize)
                    .ok_or(TransportError::UnknownParty(to))?;
                tx.send(msg.clone()).map_err(|_| TransportError::Closed)
            }
            None => {
                for (i, tx) in self.senders.iter().enumerate() {
                    if i as u16 != self.my_index {
                        tx.send(msg.clone()).map_err(|_| TransportError::Closed)?;
                    }
                }
                Ok(())
            }
        }
    }

    fn recv(&self, timeout: Duration) -> Result<WireMessage, TransportError> {
        let rx = self.rx.lock().map_err(|_| TransportError::Closed)?;
        match rx.recv_timeout(timeout) {
            Ok(m) => Ok(m),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(TransportError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(TransportError::Closed),
        }
    }
}

// ---------------------------------------------------------------- tcp

type HmacSha256 = Hmac<Sha256>;

/// The JSON line on the wire.
#[derive(Serialize, Deserialize)]
struct Frame {
    v: u8,
    session: String,
    from: u16,
    to: Option<u16>,
    broadcast: bool,
    body: String,
    mac: String,
}

fn mac_input(session: &str, from: u16, to: Option<u16>, broadcast: bool, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(session.len() + body.len() + 16);
    v.extend_from_slice(b"keel-tss-frame-v1\n");
    v.extend_from_slice(session.as_bytes());
    v.push(b'\n');
    v.extend_from_slice(&from.to_be_bytes());
    v.push(u8::from(broadcast));
    match to {
        Some(t) => {
            v.push(1);
            v.extend_from_slice(&t.to_be_bytes());
        }
        None => v.push(0),
    }
    v.extend_from_slice(&(body.len() as u64).to_be_bytes());
    v.extend_from_slice(body);
    v
}

fn compute_mac(secret: &[u8], input: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(input);
    mac.finalize().into_bytes().to_vec()
}

fn verify_mac(secret: &[u8], input: &[u8], tag: &[u8]) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(input);
    mac.verify_slice(tag).is_ok()
}

/// Encode one message as a JSON line (without the trailing newline).
pub fn encode_frame(secret: &[u8], msg: &WireMessage) -> String {
    let mac = compute_mac(
        secret,
        &mac_input(&msg.session, msg.from, msg.to, msg.broadcast, &msg.body),
    );
    let f = Frame {
        v: 1,
        session: msg.session.clone(),
        from: msg.from,
        to: msg.to,
        broadcast: msg.broadcast,
        body: hex::encode(&msg.body),
        mac: hex::encode(mac),
    };
    serde_json::to_string(&f).unwrap_or_default()
}

/// Decode and authenticate one JSON line.
pub fn decode_frame(secret: &[u8], line: &str) -> Result<WireMessage, TransportError> {
    let f: Frame =
        serde_json::from_str(line).map_err(|e| TransportError::BadFrame(e.to_string()))?;
    if f.v != 1 {
        return Err(TransportError::BadFrame(format!("version {}", f.v)));
    }
    let body = hex::decode(&f.body).map_err(|e| TransportError::BadFrame(e.to_string()))?;
    let tag = hex::decode(&f.mac).map_err(|e| TransportError::BadFrame(e.to_string()))?;
    if !verify_mac(
        secret,
        &mac_input(&f.session, f.from, f.to, f.broadcast, &body),
        &tag,
    ) {
        return Err(TransportError::BadFrame("bad mac".into()));
    }
    Ok(WireMessage {
        session: f.session,
        from: f.from,
        to: f.to,
        broadcast: f.broadcast,
        body,
    })
}

/// TCP mesh: this party listens on `peers[my_index]` and dials the others
/// lazily on first send. Connections are one-directional (dialer → listener)
/// and re-dialed after any write error.
pub struct TcpTransport {
    my_index: u16,
    peers: Vec<SocketAddr>,
    secret: Vec<u8>,
    conns: Mutex<HashMap<u16, TcpStream>>,
    rx: Mutex<mpsc::Receiver<WireMessage>>,
    dial_timeout: Duration,
}

/// Replay filter: hashes of the last frames seen.
struct Seen {
    set: HashSet<[u8; 32]>,
    order: VecDeque<[u8; 32]>,
    cap: usize,
}

impl Seen {
    fn insert(&mut self, h: [u8; 32]) -> bool {
        if !self.set.insert(h) {
            return false;
        }
        self.order.push_back(h);
        if self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

impl TcpTransport {
    /// Bind the listener and start the accept loop. `peers[my_index]` must
    /// be a local address.
    pub fn bind(
        my_index: u16,
        peers: Vec<SocketAddr>,
        secret: Vec<u8>,
    ) -> Result<Self, TransportError> {
        let listen = *peers
            .get(my_index as usize)
            .ok_or(TransportError::UnknownParty(my_index))?;
        Self::bind_on(listen, my_index, peers, secret)
    }

    /// Like [`bind`](Self::bind) with an explicit listen address (for NAT
    /// or `0.0.0.0` setups).
    pub fn bind_on(
        listen: SocketAddr,
        my_index: u16,
        peers: Vec<SocketAddr>,
        secret: Vec<u8>,
    ) -> Result<Self, TransportError> {
        let listener = TcpListener::bind(listen)?;
        let (tx, rx) = mpsc::channel::<WireMessage>();
        let n = peers.len() as u16;
        let secret_for_accept = secret.clone();
        let seen = Arc::new(Mutex::new(Seen {
            set: HashSet::new(),
            order: VecDeque::new(),
            cap: 65_536,
        }));
        thread::Builder::new()
            .name("keel-tss-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let tx = tx.clone();
                    let secret = secret_for_accept.clone();
                    let seen = seen.clone();
                    thread::spawn(move || read_loop(stream, &secret, n, tx, seen));
                }
            })
            .map_err(TransportError::Io)?;
        Ok(Self {
            my_index,
            peers,
            secret,
            conns: Mutex::new(HashMap::new()),
            rx: Mutex::new(rx),
            dial_timeout: Duration::from_secs(5),
        })
    }

    /// Local listener address (useful when bound to port 0).
    pub fn peers(&self) -> &[SocketAddr] {
        &self.peers
    }

    fn write_to(&self, to: u16, line: &str) -> Result<(), TransportError> {
        let addr = *self
            .peers
            .get(to as usize)
            .ok_or(TransportError::UnknownParty(to))?;
        let mut conns = self.conns.lock().map_err(|_| TransportError::Closed)?;
        for attempt in 0..2 {
            if let std::collections::hash_map::Entry::Vacant(e) = conns.entry(to) {
                let s = dial(addr, self.dial_timeout)?;
                s.set_nodelay(true)?;
                e.insert(s);
            }
            let stream = conns.get_mut(&to).expect("inserted above");
            let res = stream
                .write_all(line.as_bytes())
                .and_then(|_| stream.write_all(b"\n"))
                .and_then(|_| stream.flush());
            match res {
                Ok(()) => return Ok(()),
                Err(e) => {
                    conns.remove(&to);
                    if attempt == 1 {
                        return Err(TransportError::Io(e));
                    }
                }
            }
        }
        Err(TransportError::Closed)
    }
}

/// How long a party keeps redialling a peer that is not listening yet. The
/// parties of a ceremony (and the signers after a deploy) start within a
/// minute of each other, and a refused connection must not fail the run.
const DIAL_RETRY_WINDOW: Duration = Duration::from_secs(90);

fn dial(addr: SocketAddr, per_attempt: Duration) -> std::io::Result<TcpStream> {
    let deadline = std::time::Instant::now() + DIAL_RETRY_WINDOW;
    loop {
        match TcpStream::connect_timeout(&addr, per_attempt) {
            Ok(s) => return Ok(s),
            Err(e) if std::time::Instant::now() < deadline => {
                tracing::debug!(%addr, error = %e, "peer not reachable yet; retrying");
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(e),
        }
    }
}

fn read_loop(
    stream: TcpStream,
    secret: &[u8],
    n: u16,
    tx: mpsc::Sender<WireMessage>,
    seen: Arc<Mutex<Seen>>,
) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match decode_frame(secret, &line) {
            Ok(msg) if msg.from < n => {
                let h = keel_crypto::sha256(&[line.as_bytes()]);
                let fresh = seen.lock().map(|mut s| s.insert(h)).unwrap_or(true);
                if !fresh {
                    tracing::warn!(from = msg.from, session = %msg.session, "replayed frame dropped");
                    continue;
                }
                if tx.send(msg).is_err() {
                    break;
                }
            }
            Ok(msg) => tracing::warn!(from = msg.from, "frame from out-of-range party dropped"),
            Err(e) => tracing::warn!(error = %e, "unauthenticated frame dropped"),
        }
    }
}

impl Transport for TcpTransport {
    fn my_index(&self) -> u16 {
        self.my_index
    }

    fn n(&self) -> u16 {
        self.peers.len() as u16
    }

    fn send(&self, msg: &WireMessage) -> Result<(), TransportError> {
        let line = encode_frame(&self.secret, msg);
        match msg.to {
            Some(to) => self.write_to(to, &line),
            None => {
                let mut first_err = None;
                for i in 0..self.n() {
                    if i == self.my_index {
                        continue;
                    }
                    if let Err(e) = self.write_to(i, &line) {
                        tracing::warn!(to = i, error = %e, "broadcast delivery failed");
                        first_err.get_or_insert(e);
                    }
                }
                match first_err {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
        }
    }

    fn recv(&self, timeout: Duration) -> Result<WireMessage, TransportError> {
        let rx = self.rx.lock().map_err(|_| TransportError::Closed)?;
        match rx.recv_timeout(timeout) {
            Ok(m) => Ok(m),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(TransportError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(TransportError::Closed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_reject_tampering() {
        let secret = b"shared".to_vec();
        let m = WireMessage::p2p("keygen:1", 2, 0, vec![1, 2, 3]);
        let line = encode_frame(&secret, &m);
        assert_eq!(decode_frame(&secret, &line).unwrap(), m);
        assert!(decode_frame(b"other", &line).is_err());
        let tampered = line.replace("\"from\":2", "\"from\":1");
        assert!(decode_frame(&secret, &tampered).is_err());
    }

    #[test]
    fn in_memory_broadcast_skips_self() {
        let net = InMemoryNetwork::new(3);
        net[0]
            .send(&WireMessage::broadcast("s", 0, vec![9]))
            .unwrap();
        assert!(net[0].recv(Duration::from_millis(20)).is_err());
        assert_eq!(net[1].recv(Duration::from_secs(1)).unwrap().body, vec![9]);
        assert_eq!(net[2].recv(Duration::from_secs(1)).unwrap().body, vec![9]);
    }

    #[test]
    fn tcp_mesh_delivers_authenticated_frames() {
        let listeners: Vec<TcpListener> = (0..3)
            .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
            .collect();
        let peers: Vec<SocketAddr> = listeners.iter().map(|l| l.local_addr().unwrap()).collect();
        drop(listeners);
        let secret = b"s3cret".to_vec();
        let ts: Vec<TcpTransport> = (0..3)
            .map(|i| TcpTransport::bind(i, peers.clone(), secret.clone()).unwrap())
            .collect();
        ts[1]
            .send(&WireMessage::broadcast("x", 1, b"hi".to_vec()))
            .unwrap();
        ts[2]
            .send(&WireMessage::p2p("x", 2, 0, b"yo".to_vec()))
            .unwrap();
        let mut got = [
            ts[0].recv(Duration::from_secs(5)).unwrap(),
            ts[0].recv(Duration::from_secs(5)).unwrap(),
        ];
        got.sort_by_key(|m| m.from);
        assert_eq!(got[0].body, b"hi");
        assert_eq!(got[1].body, b"yo");
        assert_eq!(ts[2].recv(Duration::from_secs(2)).unwrap().body, b"hi");
        // A peer with the wrong secret is ignored by the receivers.
        let bad = TcpTransport::bind_on(
            "127.0.0.1:0".parse().unwrap(),
            1,
            peers.clone(),
            b"wrong".to_vec(),
        )
        .unwrap();
        bad.send(&WireMessage::p2p("x", 1, 0, b"evil".to_vec()))
            .unwrap();
        assert!(matches!(
            ts[0].recv(Duration::from_millis(300)),
            Err(TransportError::Timeout)
        ));
    }
}
