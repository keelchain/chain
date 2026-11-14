//! FROST threshold Ed25519 (RFC 9591, `frost-ed25519`) for the Solana
//! vault, driven over a [`Transport`] like the CGGMP21 module.
//!
//! Keygen is the three-part FROST DKG; signing is the two-round protocol
//! with exactly `t` signers, every one of which aggregates the same
//! signature. The deposit-index derivation of `keel_chains::sol` is an
//! additive tweak `t`, so a child is signed by adding `t` to every share
//! and to the group key: the Lagrange coefficients of any signing subset
//! sum to one, so the subset reconstructs the child secret exactly, and
//! no party ever learns it. The result is a standard Ed25519 signature
//! that Solana verifies under the child key.

use crate::{
    protocol::{Mailbox, ProtocolError},
    transport::{Transport, TransportError, WireMessage},
};
use curve25519_dalek::{constants::ED25519_BASEPOINT_POINT, edwards::CompressedEdwardsY, Scalar};
use frost::{
    keys::{dkg, KeyPackage, PublicKeyPackage, SigningShare, VerifyingShare},
    round1, round2, Identifier, SigningPackage, VerifyingKey,
};
use frost_ed25519 as frost;
use keel_chains::sol;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("keygen: {0}")]
    Keygen(String),
    #[error("signing: {0}")]
    Signing(String),
    #[error("invalid key share: {0}")]
    Share(String),
    #[error("bad derivation path: {0}")]
    Path(String),
    #[error("signer set must have exactly t={t} members including this party")]
    Signers { t: u16 },
    #[error("aggregated signature does not verify under the child key")]
    Verify,
}

/// Parameters every party must agree on before keygen.
#[derive(Clone)]
pub struct KeygenParams {
    pub t: u16,
    pub n: u16,
    pub my_index: u16,
    /// Unique per ceremony (e.g. `keel-vault:SOL:epoch:1`).
    pub execution_id: Vec<u8>,
    pub timeout: Duration,
}

/// One party's FROST share of the vault key, with the group's public
/// package and the chain code the ceremony agreed on.
#[derive(Clone, Serialize, Deserialize)]
pub struct EddsaShare {
    pub index: u16,
    pub n: u16,
    pub t: u16,
    pub key_package: KeyPackage,
    pub public_key_package: PublicKeyPackage,
    pub chain_code: [u8; 32],
}

fn ident(index: u16) -> Result<Identifier, Error> {
    Identifier::try_from(index + 1).map_err(|e| Error::Share(format!("identifier: {e}")))
}

fn index_of(id: &Identifier) -> u16 {
    // Identifiers are the 1-based party index serialized little-endian.
    let b = id.serialize();
    (u16::from_le_bytes([b[0], b[1]])).saturating_sub(1)
}

/// The 32-byte vault (group) public key.
pub fn vault_public_key(share: &EddsaShare) -> [u8; 32] {
    key_bytes(share.public_key_package.verifying_key()).unwrap_or([0u8; 32])
}

fn key_bytes(vk: &VerifyingKey) -> Result<[u8; 32], Error> {
    vk.serialize()
        .map_err(|e| Error::Share(e.to_string()))?
        .try_into()
        .map_err(|_| Error::Share("verifying key is not 32 bytes".into()))
}

pub fn chain_code(share: &EddsaShare) -> [u8; 32] {
    share.chain_code
}

/// Public key of the child at `path`, as `keel_chains::sol::child` derives
/// it from the registered vault key.
pub fn child_public_key(share: &EddsaShare, path: &[u32]) -> Result<[u8; 32], Error> {
    sol::child_pubkey(&vault_public_key(share), &share.chain_code, path)
        .map_err(|e| Error::Path(e.to_string()))
}

fn session_tag(kind: &str, eid: &[u8]) -> String {
    format!("{kind}:{}", hex::encode(keel_crypto::sha256(&[eid])))
}

/// Send `mine` to every other member of `parties` under `session` and
/// collect theirs; returns every member's bytes (this party's included).
fn exchange(
    mailbox: &mut Mailbox<'_>,
    session: &str,
    parties: &[u16],
    mine: Vec<u8>,
    deadline: Instant,
) -> Result<BTreeMap<u16, Vec<u8>>, Error> {
    let transport = mailbox.transport();
    let me = transport.my_index();
    for p in parties.iter().filter(|p| **p != me) {
        transport.send(&WireMessage::p2p(session, me, *p, mine.clone()))?;
    }
    let mut got: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
    got.insert(me, mine);
    while got.len() < parties.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ProtocolError::Deadline(Duration::ZERO).into());
        }
        let m = mailbox.recv_session(session, remaining)?;
        if !parties.contains(&m.from) {
            tracing::warn!(
                from = m.from,
                session,
                "message from a party outside this session dropped"
            );
            continue;
        }
        got.entry(m.from).or_insert(m.body);
    }
    Ok(got)
}

fn decode_all<T: DeserializeOwned>(
    got: BTreeMap<u16, Vec<u8>>,
    skip: u16,
) -> Result<BTreeMap<Identifier, T>, Error> {
    let mut out = BTreeMap::new();
    for (from, body) in got {
        if from == skip {
            continue;
        }
        let v: T = serde_json::from_slice(&body)
            .map_err(|e| Error::Keygen(format!("message from {from}: {e}")))?;
        out.insert(ident(from)?, v);
    }
    Ok(out)
}

/// Run the FROST DKG among all `n` parties.
pub fn keygen(params: KeygenParams, transport: &dyn Transport) -> Result<EddsaShare, Error> {
    if transport.my_index() != params.my_index || transport.n() != params.n {
        return Err(Error::Keygen(
            "transport index/size disagree with params".into(),
        ));
    }
    if params.t < 2 || params.t > params.n {
        return Err(Error::Keygen("need 2 <= t <= n".into()));
    }
    let me = params.my_index;
    let parties: Vec<u16> = (0..params.n).collect();
    let deadline = Instant::now() + params.timeout;
    let mut mailbox = Mailbox::new(transport);
    let rng = rand::rngs::OsRng;

    let (r1_secret, r1_package) = dkg::part1(ident(me)?, params.n, params.t, rng)
        .map_err(|e| Error::Keygen(e.to_string()))?;
    let body = serde_json::to_vec(&r1_package).map_err(|e| Error::Keygen(e.to_string()))?;
    let got = exchange(
        &mut mailbox,
        &session_tag("frost-dkg1", &params.execution_id),
        &parties,
        body,
        deadline,
    )?;
    let r1_packages: BTreeMap<Identifier, dkg::round1::Package> = decode_all(got, me)?;

    let (r2_secret, r2_packages) =
        dkg::part2(r1_secret, &r1_packages).map_err(|e| Error::Keygen(e.to_string()))?;
    // Round 2 is one private package per peer.
    let session2 = session_tag("frost-dkg2", &params.execution_id);
    for (to, pkg) in &r2_packages {
        let body = serde_json::to_vec(pkg).map_err(|e| Error::Keygen(e.to_string()))?;
        transport.send(&WireMessage::p2p(&session2, me, index_of(to), body))?;
    }
    let mut r2_received: BTreeMap<Identifier, dkg::round2::Package> = BTreeMap::new();
    while r2_received.len() + 1 < parties.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ProtocolError::Deadline(Duration::ZERO).into());
        }
        let m = mailbox.recv_session(&session2, remaining)?;
        let pkg: dkg::round2::Package = serde_json::from_slice(&m.body)
            .map_err(|e| Error::Keygen(format!("round 2 from {}: {e}", m.from)))?;
        r2_received.insert(ident(m.from)?, pkg);
    }
    let (key_package, public_key_package) = dkg::part3(&r2_secret, &r1_packages, &r2_received)
        .map_err(|e| Error::Keygen(e.to_string()))?;
    let chain_code = keel_crypto::sha256(&[
        b"keel-frost-ed25519-chain-code",
        &params.execution_id,
        &key_bytes(public_key_package.verifying_key())?,
    ]);
    Ok(EddsaShare {
        index: me,
        n: params.n,
        t: params.t,
        key_package,
        public_key_package,
        chain_code,
    })
}

fn scalar_of(bytes: &[u8]) -> Result<Scalar, Error> {
    let b: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Share("scalar is not 32 bytes".into()))?;
    Option::<Scalar>::from(Scalar::from_canonical_bytes(b))
        .ok_or_else(|| Error::Share("non-canonical scalar".into()))
}

fn point_of(bytes: &[u8]) -> Result<curve25519_dalek::EdwardsPoint, Error> {
    let b: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Share("point is not 32 bytes".into()))?;
    CompressedEdwardsY(b)
        .decompress()
        .ok_or_else(|| Error::Share("not an ed25519 point".into()))
}

/// The share and group package shifted by the child tweak of `path`.
fn tweaked(
    share: &EddsaShare,
    path: &[u32],
) -> Result<(KeyPackage, PublicKeyPackage, [u8; 32]), Error> {
    let vault = vault_public_key(share);
    let (t, child) =
        sol::child(&vault, &share.chain_code, path).map_err(|e| Error::Path(e.to_string()))?;
    let shift = t * ED25519_BASEPOINT_POINT;
    let s = scalar_of(&share.key_package.signing_share().serialize())? + t;
    let signing_share =
        SigningShare::deserialize(&s.to_bytes()).map_err(|e| Error::Share(e.to_string()))?;
    let mut shares = BTreeMap::new();
    for (id, vs) in share.public_key_package.verifying_shares() {
        let p = point_of(&vs.serialize().map_err(|e| Error::Share(e.to_string()))?)? + shift;
        let vs2 = VerifyingShare::deserialize(&p.compress().to_bytes())
            .map_err(|e| Error::Share(e.to_string()))?;
        shares.insert(*id, vs2);
    }
    let vk = VerifyingKey::deserialize(&child).map_err(|e| Error::Share(e.to_string()))?;
    let my_vs = shares
        .get(share.key_package.identifier())
        .cloned()
        .ok_or_else(|| Error::Share("own verifying share missing".into()))?;
    let kp = KeyPackage::new(
        *share.key_package.identifier(),
        signing_share,
        my_vs,
        vk,
        *share.key_package.min_signers(),
    );
    Ok((kp, PublicKeyPackage::new(shares, vk), child))
}

/// Sign `message` with the child key at `path`. `signers` are the keygen
/// indexes of the exactly-`t` participating parties (this party
/// included); every signer must call this with the same arguments.
pub fn sign(
    share: &EddsaShare,
    path: &[u32],
    message: &[u8],
    signers: &[u16],
    session_id: &[u8],
    transport: &dyn Transport,
    timeout: Duration,
) -> Result<[u8; 64], Error> {
    let mut mailbox = Mailbox::new(transport);
    sign_with_mailbox(
        share,
        path,
        message,
        signers,
        session_id,
        &mut mailbox,
        timeout,
    )
}

/// [`sign`] over an existing [`Mailbox`].
pub fn sign_with_mailbox(
    share: &EddsaShare,
    path: &[u32],
    message: &[u8],
    signers: &[u16],
    session_id: &[u8],
    mailbox: &mut Mailbox<'_>,
    timeout: Duration,
) -> Result<[u8; 64], Error> {
    let t = share.t;
    let me = mailbox.transport().my_index();
    if signers.len() != t as usize || !signers.contains(&me) {
        return Err(Error::Signers { t });
    }
    let deadline = Instant::now() + timeout;
    let (key_package, public_key_package, child) = tweaked(share, path)?;
    let mut rng = rand::rngs::OsRng;

    let (nonces, commitments) = round1::commit(key_package.signing_share(), &mut rng);
    let body = serde_json::to_vec(&commitments).map_err(|e| Error::Signing(e.to_string()))?;
    let got = exchange(
        mailbox,
        &session_tag("frost1", session_id),
        signers,
        body,
        deadline,
    )?;
    let mut all: BTreeMap<Identifier, round1::SigningCommitments> = BTreeMap::new();
    for (from, b) in got {
        let c: round1::SigningCommitments = serde_json::from_slice(&b)
            .map_err(|e| Error::Signing(format!("commitment from {from}: {e}")))?;
        all.insert(ident(from)?, c);
    }
    let package = SigningPackage::new(all, message);
    let my_share =
        round2::sign(&package, &nonces, &key_package).map_err(|e| Error::Signing(e.to_string()))?;
    let body = serde_json::to_vec(&my_share).map_err(|e| Error::Signing(e.to_string()))?;
    let got = exchange(
        mailbox,
        &session_tag("frost2", session_id),
        signers,
        body,
        deadline,
    )?;
    let mut shares: BTreeMap<Identifier, round2::SignatureShare> = BTreeMap::new();
    for (from, b) in got {
        let s: round2::SignatureShare = serde_json::from_slice(&b)
            .map_err(|e| Error::Signing(format!("signature share from {from}: {e}")))?;
        shares.insert(ident(from)?, s);
    }
    let sig = frost::aggregate(&package, &shares, &public_key_package)
        .map_err(|e| Error::Signing(e.to_string()))?;
    let bytes: [u8; 64] = sig
        .serialize()
        .map_err(|e| Error::Signing(e.to_string()))?
        .try_into()
        .map_err(|_| Error::Signing("signature is not 64 bytes".into()))?;
    if !sol::verify(&child, message, &bytes) {
        return Err(Error::Verify);
    }
    Ok(bytes)
}
