//! The signer behind the vault key: the `keel-tss serve` daemon over HTTP,
//! or a single-key development signer that derives BIP32 children from a
//! seed (only for regtest/devnet runs and tests). Both answer for the
//! secp256k1 chains (ECDSA over a digest) and for Solana (Ed25519 over a
//! message, under `keel_chains::sol`'s child derivation).

use async_trait::async_trait;
use bitcoin::bip32::{ChildNumber, Xpriv};
use curve25519_dalek::scalar::Scalar;
use keel_chains::{policy::SignContext, sol};
use secp256k1::{Message, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TssSignature {
    pub r: [u8; 32],
    pub s: [u8; 32],
    pub v: u8,
}

impl TssSignature {
    pub fn compact(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&self.r);
        out[32..].copy_from_slice(&self.s);
        out
    }
}

#[async_trait]
pub trait TssClient: Send + Sync {
    /// Sign `digest` with the non-hardened child at `path`. `context`
    /// describes the transaction the digest belongs to; a policy-enforcing
    /// signer refuses requests without it.
    async fn sign(
        &self,
        digest: [u8; 32],
        path: &[u32],
        context: Option<&SignContext>,
    ) -> anyhow::Result<TssSignature>;

    /// Ed25519-sign `message` with the Solana child at `path`.
    async fn sign_message(
        &self,
        message: &[u8],
        path: &[u32],
        context: Option<&SignContext>,
    ) -> anyhow::Result<[u8; 64]>;
}

#[derive(Serialize)]
struct SignRequest<'a> {
    digest: String,
    path: &'a [u32],
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a SignContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scheme: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Deserialize)]
struct SignResponse {
    #[serde(default)]
    r: String,
    #[serde(default)]
    s: String,
    #[serde(default)]
    v: u8,
    #[serde(default)]
    signature: String,
}

pub struct HttpTss {
    url: String,
    client: reqwest::Client,
}

impl HttpTss {
    pub fn new(base: &str) -> Self {
        Self {
            url: format!("{}/sign", base.trim_end_matches('/')),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl TssClient for HttpTss {
    async fn sign(
        &self,
        digest: [u8; 32],
        path: &[u32],
        context: Option<&SignContext>,
    ) -> anyhow::Result<TssSignature> {
        let resp = self
            .client
            .post(&self.url)
            .json(&SignRequest {
                digest: hex::encode(digest),
                path,
                context,
                scheme: None,
                message: None,
            })
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "tss: {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }
        let r: SignResponse = resp.json().await?;
        let to32 = |s: &str| -> anyhow::Result<[u8; 32]> {
            hex::decode(s)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("tss returned a non-32-byte scalar"))
        };
        Ok(TssSignature {
            r: to32(&r.r)?,
            s: to32(&r.s)?,
            v: r.v,
        })
    }

    async fn sign_message(
        &self,
        message: &[u8],
        path: &[u32],
        context: Option<&SignContext>,
    ) -> anyhow::Result<[u8; 64]> {
        let digest: [u8; 32] = sha2::Sha256::digest(message).into();
        let resp = self
            .client
            .post(&self.url)
            .json(&SignRequest {
                digest: hex::encode(digest),
                path,
                context,
                scheme: Some("eddsa"),
                message: Some(hex::encode(message)),
            })
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "tss: {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            );
        }
        let r: SignResponse = resp.json().await?;
        hex::decode(&r.signature)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("tss returned a non-64-byte signature"))
    }
}

/// Development signer: one BIP32 master key, children derived the same
/// way the chain derives deposit addresses. Register its
/// `(public_key, chain_code)` as the vault to use it end to end.
pub struct LocalSigner {
    master: Xpriv,
    /// The Solana vault: a raw Ed25519 scalar, a nonce prefix and a chain
    /// code, all drawn from the same seed.
    ed_scalar: Scalar,
    ed_prefix: [u8; 32],
    ed_chain_code: [u8; 32],
}

impl LocalSigner {
    pub fn from_seed(seed: &[u8]) -> anyhow::Result<Self> {
        let wide: [u8; 64] = sha2::Sha512::new()
            .chain_update(b"keel-local-ed25519")
            .chain_update(seed)
            .finalize()
            .into();
        Ok(Self {
            master: Xpriv::new_master(bitcoin::Network::Regtest, seed)?,
            ed_scalar: Scalar::from_bytes_mod_order_wide(&wide),
            ed_prefix: keel_crypto::sha256(&[b"keel-local-ed25519-prefix", seed]),
            ed_chain_code: keel_crypto::sha256(&[b"keel-local-ed25519-chain-code", seed]),
        })
    }

    /// The Ed25519 vault key (Solana).
    pub fn ed25519_public_key(&self) -> [u8; 32] {
        sol::pubkey_of_scalar(&self.ed_scalar)
    }

    pub fn ed25519_chain_code(&self) -> [u8; 32] {
        self.ed_chain_code
    }

    /// The vault key and chain code to register for `chain`.
    pub fn vault_key(&self, chain: keel_actions::Chain) -> (Vec<u8>, [u8; 32]) {
        if chain.is_ed25519() {
            (self.ed25519_public_key().to_vec(), self.ed_chain_code)
        } else {
            (self.public_key().to_vec(), self.chain_code())
        }
    }

    pub fn sign_message_sync(&self, message: &[u8], path: &[u32]) -> anyhow::Result<[u8; 64]> {
        let (t, _) = sol::child(&self.ed25519_public_key(), &self.ed_chain_code, path)?;
        Ok(sol::sign_with_scalar(
            &(self.ed_scalar + t),
            &self.ed_prefix,
            message,
        ))
    }

    pub fn public_key(&self) -> [u8; 33] {
        self.master
            .private_key
            .public_key(&Secp256k1::new())
            .serialize()
    }

    pub fn chain_code(&self) -> [u8; 32] {
        self.master.chain_code.to_bytes()
    }

    fn child(&self, path: &[u32]) -> anyhow::Result<SecretKey> {
        let secp = Secp256k1::new();
        let path: Vec<ChildNumber> = path
            .iter()
            .map(|i| ChildNumber::from_normal_idx(*i))
            .collect::<Result<_, _>>()?;
        Ok(self.master.derive_priv(&secp, &path)?.private_key)
    }

    pub fn sign_sync(&self, digest: [u8; 32], path: &[u32]) -> anyhow::Result<TssSignature> {
        let secp = Secp256k1::new();
        let sk = self.child(path)?;
        let sig = secp.sign_ecdsa_recoverable(&Message::from_digest(digest), &sk);
        let (rid, compact) = sig.serialize_compact();
        let mut r = [0u8; 32];
        let mut s = [0u8; 32];
        r.copy_from_slice(&compact[..32]);
        s.copy_from_slice(&compact[32..]);
        Ok(TssSignature {
            r,
            s,
            v: rid.to_i32() as u8,
        })
    }
}

/// Development stand-in for a client's signing service: answers
/// `POST /sign` like `keel-tss serve` does, from one seed and without the
/// signing policy. For local end-to-end runs only.
pub async fn serve_local(seed: &[u8], listen: std::net::SocketAddr) -> anyhow::Result<()> {
    use axum::{routing::post, Json, Router};
    use std::sync::Arc;

    #[derive(serde::Deserialize)]
    struct Req {
        digest: String,
        path: Vec<u32>,
        #[serde(default)]
        scheme: Option<String>,
        #[serde(default)]
        message: Option<String>,
    }
    let signer = Arc::new(LocalSigner::from_seed(seed)?);
    tracing::info!(
        %listen,
        public_key = hex::encode(signer.public_key()),
        chain_code = hex::encode(signer.chain_code()),
        ed25519_public_key = hex::encode(signer.ed25519_public_key()),
        ed25519_chain_code = hex::encode(signer.ed25519_chain_code()),
        "development signer serving /sign"
    );
    let app = Router::new().route(
        "/sign",
        post(move |Json(req): Json<Req>| {
            let signer = signer.clone();
            async move {
                let bad = |m: String| (axum::http::StatusCode::BAD_REQUEST, m);
                if req.scheme.as_deref() == Some("eddsa") {
                    let message = hex::decode(req.message.unwrap_or_default())
                        .map_err(|e| bad(format!("message: {e}")))?;
                    let sig = signer
                        .sign_message_sync(&message, &req.path)
                        .map_err(|e| bad(e.to_string()))?;
                    return Ok::<_, (axum::http::StatusCode, String)>(Json(
                        serde_json::json!({ "signature": hex::encode(sig) }),
                    ));
                }
                let digest: [u8; 32] = hex::decode(&req.digest)
                    .ok()
                    .and_then(|d| d.try_into().ok())
                    .ok_or_else(|| bad("digest".to_string()))?;
                let sig = signer
                    .sign_sync(digest, &req.path)
                    .map_err(|e| bad(e.to_string()))?;
                Ok(Json(serde_json::json!({
                    "r": hex::encode(sig.r),
                    "s": hex::encode(sig.s),
                    "v": sig.v,
                })))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[async_trait]
impl TssClient for LocalSigner {
    async fn sign(
        &self,
        digest: [u8; 32],
        path: &[u32],
        _context: Option<&SignContext>,
    ) -> anyhow::Result<TssSignature> {
        self.sign_sync(digest, path)
    }

    async fn sign_message(
        &self,
        message: &[u8],
        path: &[u32],
        _context: Option<&SignContext>,
    ) -> anyhow::Result<[u8; 64]> {
        self.sign_message_sync(message, path)
    }
}

/// Build the client named by `tss_url` (`http://…` or `local:<hex seed>`).
pub fn client_from_url(url: &str) -> anyhow::Result<Box<dyn TssClient>> {
    if let Some(seed) = url.strip_prefix("local:") {
        tracing::warn!("using the single-key development signer; never do this with real funds");
        return Ok(Box::new(LocalSigner::from_seed(&hex::decode(
            seed.trim(),
        )?)?));
    }
    Ok(Box::new(HttpTss::new(url)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_signer_matches_keel_chains_derivation() {
        let signer = LocalSigner::from_seed(&[7u8; 32]).unwrap();
        let path = keel_chains::hd::deposit_path(keel_actions::Chain::Bitcoin, 5);
        let child =
            keel_chains::hd::child_pubkey(&signer.public_key(), &signer.chain_code(), &path)
                .unwrap();
        let digest = [9u8; 32];
        let sig = signer.sign_sync(digest, &path).unwrap();
        assert_eq!(
            keel_chains::eth::recovery_id(&digest, &sig.compact(), &child.public_key).unwrap(),
            sig.v
        );
        let secp = Secp256k1::verification_only();
        let pk = secp256k1::PublicKey::from_slice(&child.public_key).unwrap();
        secp.verify_ecdsa(
            &Message::from_digest(digest),
            &secp256k1::ecdsa::Signature::from_compact(&sig.compact()).unwrap(),
            &pk,
        )
        .unwrap();
    }

    #[test]
    fn local_signer_signs_solana_children_keel_chains_derives() {
        let signer = LocalSigner::from_seed(&[7u8; 32]).unwrap();
        let path = keel_chains::hd::deposit_path(keel_actions::Chain::Solana, 3);
        let child = sol::child_pubkey(
            &signer.ed25519_public_key(),
            &signer.ed25519_chain_code(),
            &path,
        )
        .unwrap();
        let msg = b"solana message";
        let sig = signer.sign_message_sync(msg, &path).unwrap();
        assert!(sol::verify(&child, msg, &sig));
        assert!(!sol::verify(&signer.ed25519_public_key(), msg, &sig));
        let (pk, cc) = signer.vault_key(keel_actions::Chain::Solana);
        assert_eq!(pk.len(), 32);
        assert_eq!(cc, signer.ed25519_chain_code());
        assert_eq!(signer.vault_key(keel_actions::Chain::Bitcoin).0.len(), 33);
    }
}
