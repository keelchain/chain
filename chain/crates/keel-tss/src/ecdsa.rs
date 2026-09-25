//! CGGMP21 threshold ECDSA on secp256k1 with SLIP-10/BIP32 non-hardened
//! child derivation, driven over a [`Transport`].
//!
//! Keygen is two protocols run back to back on the same link: the
//! threshold DKG (with `hd_wallet = true`, which also agrees on a chain
//! code) and the auxiliary-info generation (Paillier moduli and ring-
//! Pedersen parameters, the slow part). Signing is the (3+1)-round
//! protocol with exactly `t` signers; the derivation path is folded into
//! the signature as an additive shift so no party ever learns a child
//! secret either.

use crate::{
    protocol::{self, Mailbox, ProtocolError},
    transport::Transport,
};
use cggmp21::{
    generic_ec::{Point, Scalar},
    hd_wallet::{self, HdWallet as _, NonHardenedIndex},
    key_share::{AnyKeyShare as _, AuxInfo},
    security_level::SecurityLevel128,
    supported_curves::Secp256k1,
    DataToSign, ExecutionId, PregeneratedPrimes,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub type Curve = Secp256k1;
pub type Level = SecurityLevel128;
pub type KeyShare = cggmp21::KeyShare<Curve, Level>;
pub type IncompleteKeyShare = cggmp21::IncompleteKeyShare<Curve>;
pub type Primes = PregeneratedPrimes<Level>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
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
    #[error("signature does not recover the derived key")]
    Recovery,
}

/// Parameters every party must agree on before keygen.
#[derive(Clone)]
pub struct KeygenParams {
    pub t: u16,
    pub n: u16,
    pub my_index: u16,
    /// Unique per ceremony (e.g. `keel-vault:BTC:epoch:3`).
    pub execution_id: Vec<u8>,
    /// Safe primes for this party's Paillier key; generated when `None`
    /// (slow: two 1536-bit safe primes).
    pub primes: Option<Primes>,
    pub timeout: Duration,
}

/// An ECDSA signature with the recovery id of the derived child key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcdsaSignature {
    #[serde(with = "hex32")]
    pub r: [u8; 32],
    #[serde(with = "hex32")]
    pub s: [u8; 32],
    pub v: u8,
}

impl EcdsaSignature {
    pub fn compact(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&self.r);
        out[32..].copy_from_slice(&self.s);
        out
    }
}

mod hex32 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        hex::encode(v).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(s).map_err(serde::de::Error::custom)?;
        v.try_into()
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}

fn session_tag(kind: &str, eid: &[u8]) -> String {
    format!("{kind}:{}", hex::encode(keel_crypto::sha256(&[eid])))
}

/// Generate safe primes for the auxiliary info (call once, cache on disk).
pub fn generate_primes() -> Primes {
    Primes::generate(&mut rand::rngs::OsRng)
}

/// Run the DKG + aux-info generation among all `n` parties.
pub fn keygen(params: KeygenParams, transport: &dyn Transport) -> Result<KeyShare, Error> {
    if transport.my_index() != params.my_index || transport.n() != params.n {
        return Err(Error::Keygen(
            "transport index/size disagree with params".into(),
        ));
    }
    if params.t < 2 || params.t > params.n {
        return Err(Error::Keygen("need 2 <= t <= n".into()));
    }
    let mut rng = rand::rngs::OsRng;
    let mut mailbox = Mailbox::new(transport);
    let parties: Vec<u16> = (0..params.n).collect();

    let core: IncompleteKeyShare = {
        let eid_bytes = [b"keygen:".as_slice(), &params.execution_id].concat();
        let eid = ExecutionId::new(&eid_bytes);
        let mut sm = cggmp21::keygen::<Curve>(eid, params.my_index, params.n)
            .set_threshold(params.t)
            .hd_wallet(true)
            .into_state_machine(&mut rng);
        protocol::run(
            &mut sm,
            &mut mailbox,
            &session_tag("keygen", &params.execution_id),
            &parties,
            params.timeout,
        )?
        .map_err(|e| Error::Keygen(e.to_string()))?
    };
    tracing::info!(
        index = params.my_index,
        "dkg done, generating auxiliary info"
    );

    let primes = params.primes.unwrap_or_else(|| Primes::generate(&mut rng));
    let aux: AuxInfo<Level> = {
        let eid_bytes = [b"aux:".as_slice(), &params.execution_id].concat();
        let eid = ExecutionId::new(&eid_bytes);
        let mut sm = cggmp21::aux_info_gen(eid, params.my_index, params.n, primes)
            .into_state_machine(&mut rng);
        protocol::run(
            &mut sm,
            &mut mailbox,
            &session_tag("aux", &params.execution_id),
            &parties,
            params.timeout,
        )?
        .map_err(|e| Error::Keygen(e.to_string()))?
    };
    KeyShare::from_parts((core, aux)).map_err(|e| Error::Share(e.to_string()))
}

/// Compressed vault (master) public key.
pub fn vault_public_key(share: &KeyShare) -> [u8; 33] {
    point_bytes(&share.shared_public_key.into_inner())
}

/// The BIP32 chain code agreed at keygen (`None` for a non-HD share).
pub fn chain_code(share: &KeyShare) -> Option<[u8; 32]> {
    share.chain_code
}

fn point_bytes(p: &Point<Curve>) -> [u8; 33] {
    let enc = p.to_bytes(true);
    let mut out = [0u8; 33];
    out.copy_from_slice(enc.as_bytes());
    out
}

fn parse_path(path: &[u32]) -> Result<Vec<NonHardenedIndex>, Error> {
    path.iter()
        .map(|i| NonHardenedIndex::try_from(*i).map_err(|e| Error::Path(format!("{i}: {e}"))))
        .collect()
}

/// Public key of the non-hardened child at `path`, derived exactly as
/// `keel_chains::hd::child_pubkey` does from `(vault_public_key, chain_code)`.
pub fn child_public_key(share: &KeyShare, path: &[u32]) -> Result<[u8; 33], Error> {
    let xpub = share
        .extended_public_key()
        .ok_or_else(|| Error::Share("share is not HD-capable".into()))?;
    let child = hd_wallet::Slip10::derive_child_public_key_with_path(&xpub, parse_path(path)?);
    Ok(point_bytes(&child.public_key))
}

/// Sign a 32-byte digest with the child key at `path`. `signers` are the
/// keygen indexes of the exactly-`t` participating parties (this party
/// included); every signer must call this with the same arguments and
/// `session_id`.
pub fn sign(
    share: &KeyShare,
    path: &[u32],
    digest: [u8; 32],
    signers: &[u16],
    session_id: &[u8],
    transport: &dyn Transport,
    timeout: Duration,
) -> Result<EcdsaSignature, Error> {
    let mut mailbox = Mailbox::new(transport);
    sign_with_mailbox(
        share,
        path,
        digest,
        signers,
        session_id,
        &mut mailbox,
        timeout,
    )
}

/// [`sign`] over an existing [`Mailbox`], so messages that arrived before
/// the caller learnt about the session are not lost.
pub fn sign_with_mailbox(
    share: &KeyShare,
    path: &[u32],
    digest: [u8; 32],
    signers: &[u16],
    session_id: &[u8],
    mailbox: &mut Mailbox<'_>,
    timeout: Duration,
) -> Result<EcdsaSignature, Error> {
    let t = share.min_signers();
    let me = mailbox.transport().my_index();
    if signers.len() != t as usize || !signers.contains(&me) {
        return Err(Error::Signers { t });
    }
    let local = signers
        .iter()
        .position(|s| *s == me)
        .expect("checked above") as u16;
    let indexes = parse_path(path)?;
    let child = child_public_key(share, path)?;

    let mut rng = rand::rngs::OsRng;
    let eid_bytes = [b"sign:".as_slice(), session_id].concat();
    let eid = ExecutionId::new(&eid_bytes);
    let builder = cggmp21::signing(eid, local, signers, share)
        .set_derivation_path(indexes)
        .map_err(|e| Error::Path(e.to_string()))?;
    let data = DataToSign::from_scalar(Scalar::<Curve>::from_be_bytes_mod_order(digest));
    let mut sm = builder.sign_sync(&mut rng, data);
    let sig = protocol::run(
        &mut sm,
        mailbox,
        &session_tag("sign", session_id),
        signers,
        timeout,
    )?
    .map_err(|e| Error::Signing(e.to_string()))?
    .normalize_s();
    let mut compact = [0u8; 64];
    sig.write_to_slice(&mut compact);
    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r.copy_from_slice(&compact[..32]);
    s.copy_from_slice(&compact[32..]);
    let v = recovery_id(&digest, &compact, &child)?;
    Ok(EcdsaSignature { r, s, v })
}

/// Which recovery id makes `sig64` over `digest` recover `expected`.
pub fn recovery_id(digest: &[u8; 32], sig64: &[u8; 64], expected: &[u8; 33]) -> Result<u8, Error> {
    use secp256k1::{
        ecdsa::{RecoverableSignature, RecoveryId},
        Message, PublicKey, SECP256K1,
    };
    let msg = Message::from_digest(*digest);
    let expected = PublicKey::from_slice(expected).map_err(|e| Error::Share(e.to_string()))?;
    for v in 0..2i32 {
        let rid = RecoveryId::from_i32(v).map_err(|e| Error::Signing(e.to_string()))?;
        if let Ok(rs) = RecoverableSignature::from_compact(sig64, rid) {
            if let Ok(pk) = SECP256K1.recover_ecdsa(&msg, &rs) {
                if pk == expected {
                    return Ok(v as u8);
                }
            }
        }
    }
    Err(Error::Recovery)
}

/// Verify a compact signature against a compressed public key.
pub fn verify(digest: &[u8; 32], sig64: &[u8; 64], pubkey: &[u8; 33]) -> bool {
    use secp256k1::{ecdsa::Signature, Message, PublicKey, SECP256K1};
    let (Ok(sig), Ok(pk)) = (
        Signature::from_compact(sig64),
        PublicKey::from_slice(pubkey),
    ) else {
        return false;
    };
    SECP256K1
        .verify_ecdsa(&Message::from_digest(*digest), &sig, &pk)
        .is_ok()
}
