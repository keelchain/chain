//! 3-of-4 keygen and signing in-process over the in-memory transport.
//!
//! The Paillier safe primes come from `tests/fixtures/primes.json` when it
//! exists (generate with `keel-tss gen-primes`, one entry per party) so the
//! test does not spend minutes on prime search.
#![allow(clippy::unwrap_used, clippy::disallowed_types)]

use keel_actions::Chain;
use keel_tss::{
    ecdsa::{self, KeyShare, KeygenParams, Primes},
    store,
    transport::{InMemoryNetwork, Transport},
};
use std::{path::Path, thread, time::Duration};

const N: u16 = 4;
const T: u16 = 3;

fn fixture_primes() -> Option<Vec<Primes>> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/primes.json");
    let bytes = std::fs::read(p).ok()?;
    let v: Vec<Primes> = serde_json::from_slice(&bytes).ok()?;
    (v.len() >= N as usize).then_some(v)
}

fn keygen_all() -> Vec<KeyShare> {
    let primes = fixture_primes();
    let net = InMemoryNetwork::new(N);
    let handles: Vec<_> = net
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let primes = primes.as_ref().map(|p| p[i].clone());
            thread::spawn(move || {
                let params = KeygenParams {
                    t: T,
                    n: N,
                    my_index: i as u16,
                    execution_id: b"keel-tss-test-ceremony".to_vec(),
                    primes,
                    timeout: Duration::from_secs(1800),
                };
                ecdsa::keygen(params, &t).unwrap()
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

fn sign_all(
    shares: &[KeyShare],
    signers: &[u16],
    path: &[u32],
    digest: [u8; 32],
    session: &[u8],
) -> Vec<ecdsa::EcdsaSignature> {
    let net = InMemoryNetwork::new(N);
    let handles: Vec<_> = net
        .into_iter()
        .filter(|t| signers.contains(&t.my_index()))
        .map(|t| {
            let share = shares[t.my_index() as usize].clone();
            let signers = signers.to_vec();
            let path = path.to_vec();
            let session = session.to_vec();
            thread::spawn(move || {
                ecdsa::sign(
                    &share,
                    &path,
                    digest,
                    &signers,
                    &session,
                    &t,
                    Duration::from_secs(600),
                )
                .unwrap()
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

#[test]
fn three_of_four_keygen_sign_and_store() {
    let shares = keygen_all();
    let pk = ecdsa::vault_public_key(&shares[0]);
    let cc = ecdsa::chain_code(&shares[0]).expect("hd share");
    for s in &shares {
        assert_eq!(ecdsa::vault_public_key(s), pk);
        assert_eq!(ecdsa::chain_code(s), Some(cc));
    }

    // Child derivation agrees with keel-chains (BIP32 non-hardened).
    let path = keel_chains::hd::deposit_path(Chain::Bitcoin, 7);
    let expected = keel_chains::hd::child_pubkey(&pk, &cc, &path).unwrap();
    assert_eq!(
        ecdsa::child_public_key(&shares[0], &path).unwrap(),
        expected.public_key
    );

    // Sign with parties {0, 2, 3}; verify under the keel-chains child key.
    let digest = keel_crypto::sha256(&[b"withdrawal batch 1"]);
    let sigs = sign_all(&shares, &[0, 2, 3], &path, digest, b"session-1");
    assert_eq!(sigs.len(), 3);
    for sig in &sigs {
        assert_eq!(*sig, sigs[0]);
        assert!(ecdsa::verify(&digest, &sig.compact(), &expected.public_key));
        assert!(!ecdsa::verify(&digest, &sig.compact(), &pk));
        assert_eq!(
            ecdsa::recovery_id(&digest, &sig.compact(), &expected.public_key).unwrap(),
            sig.v
        );
    }
    // ETH-style child on another chain index, different signer subset.
    let path2 = keel_chains::hd::deposit_path(Chain::Ethereum, 1);
    let child2 = keel_chains::hd::child_pubkey(&pk, &cc, &path2).unwrap();
    let sigs2 = sign_all(&shares, &[1, 2, 3], &path2, digest, b"session-2");
    assert!(ecdsa::verify(
        &digest,
        &sigs2[0].compact(),
        &child2.public_key
    ));
    // The recovery id recovers the child address as keel-chains computes it.
    assert_eq!(
        keel_chains::eth::recovery_id(&digest, &sigs2[0].compact(), &child2.public_key).unwrap(),
        sigs2[0].v
    );

    // Wrong signer set size is refused up front.
    let net = InMemoryNetwork::new(N);
    assert!(matches!(
        ecdsa::sign(
            &shares[0],
            &path,
            digest,
            &[0, 1],
            b"x",
            &net[0],
            Duration::from_secs(1)
        ),
        Err(ecdsa::Error::Signers { t: 3 })
    ));

    // Encrypted storage round-trips and a wrong passphrase fails.
    let doc = store::encrypt_share(&shares[1], "correct horse", 1000).unwrap();
    let back = store::decrypt_share(&doc, "correct horse").unwrap();
    assert_eq!(ecdsa::vault_public_key(&back), pk);
    assert_eq!(back.i, 1);
    assert!(matches!(
        store::decrypt_share(&doc, "wrong"),
        Err(store::StoreError::Decrypt)
    ));
    let mut tampered: serde_json::Value = serde_json::from_str(&doc).unwrap();
    tampered["header"]["index"] = serde_json::json!(2);
    assert!(store::decrypt_share(&tampered.to_string(), "correct horse").is_err());
    let dir = std::env::temp_dir().join(format!("keel-tss-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("share.enc");
    store::save_share(&file, &shares[2], "pw").unwrap();
    assert_eq!(store::read_header(&file).unwrap().index, 2);
    assert_eq!(
        ecdsa::vault_public_key(&store::load_share(&file, "pw").unwrap()),
        pk
    );
    assert!(store::load_share(&file, "pw2").is_err());
    let _ = std::fs::remove_dir_all(dir);
}
