//! 3-of-4 FROST Ed25519 keygen and signing in-process over the in-memory
//! transport: every signer aggregates the same signature, it verifies
//! under the child key `keel-chains` derives from the registered vault
//! key, and the encrypted store round-trips the share.
#![allow(clippy::unwrap_used, clippy::disallowed_types)]

use keel_actions::Chain;
use keel_tss::{
    eddsa::{self, EddsaShare, KeygenParams},
    store,
    transport::{InMemoryNetwork, Transport},
};
use std::{thread, time::Duration};

const N: u16 = 4;
const T: u16 = 3;

fn keygen_all() -> Vec<EddsaShare> {
    let net = InMemoryNetwork::new(N);
    let handles: Vec<_> = net
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            thread::spawn(move || {
                let params = KeygenParams {
                    t: T,
                    n: N,
                    my_index: i as u16,
                    execution_id: b"keel-vault:SOL:epoch:1".to_vec(),
                    timeout: Duration::from_secs(120),
                };
                eddsa::keygen(params, &t).unwrap()
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

fn sign_all(
    shares: &[EddsaShare],
    signers: &[u16],
    path: &[u32],
    message: &[u8],
    session: &[u8],
) -> Vec<[u8; 64]> {
    let net = InMemoryNetwork::new(N);
    let handles: Vec<_> = net
        .into_iter()
        .filter(|t| signers.contains(&t.my_index()))
        .map(|t| {
            let share = shares[t.my_index() as usize].clone();
            let signers = signers.to_vec();
            let path = path.to_vec();
            let message = message.to_vec();
            let session = session.to_vec();
            thread::spawn(move || {
                eddsa::sign(
                    &share,
                    &path,
                    &message,
                    &signers,
                    &session,
                    &t,
                    Duration::from_secs(60),
                )
                .unwrap()
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

#[test]
fn three_of_four_frost_keygen_sign_and_store() {
    let shares = keygen_all();
    let pk = eddsa::vault_public_key(&shares[0]);
    let cc = eddsa::chain_code(&shares[0]);
    for s in &shares {
        assert_eq!(eddsa::vault_public_key(s), pk);
        assert_eq!(eddsa::chain_code(s), cc);
    }
    assert_ne!(pk, [0u8; 32]);

    // Child derivation agrees with keel-chains.
    let path = keel_chains::hd::deposit_path(Chain::Solana, 7);
    let expected = keel_chains::sol::child_pubkey(&pk, &cc, &path).unwrap();
    assert_eq!(
        eddsa::child_public_key(&shares[0], &path).unwrap(),
        expected
    );
    assert_eq!(
        keel_chains::deposit_address(Chain::Solana, keel_chains::Network::Testnet, &pk, &cc, 7)
            .unwrap(),
        keel_chains::sol::address(&expected)
    );

    // Sign a Solana message with parties {0, 2, 3}: one signature, valid
    // under the child key, verified by an independent Ed25519 verifier.
    let message = b"solana transaction message bytes".to_vec();
    let sigs = sign_all(&shares, &[0, 2, 3], &path, &message, b"session-1");
    assert_eq!(sigs.len(), 3);
    for sig in &sigs {
        assert_eq!(*sig, sigs[0]);
        assert!(keel_chains::sol::verify(&expected, &message, sig));
        assert!(!keel_chains::sol::verify(&pk, &message, sig));
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&expected).unwrap();
        vk.verify_strict(&message, &ed25519_dalek::Signature::from_bytes(sig))
            .unwrap();
    }
    // The hot address (index 0) with another subset.
    let hot_path = keel_chains::hd::deposit_path(Chain::Solana, 0);
    let hot = keel_chains::sol::child_pubkey(&pk, &cc, &hot_path).unwrap();
    let sigs2 = sign_all(&shares, &[1, 2, 3], &hot_path, &message, b"session-2");
    assert!(keel_chains::sol::verify(&hot, &message, &sigs2[0]));
    assert!(!keel_chains::sol::verify(&expected, &message, &sigs2[0]));

    // Wrong signer set size is refused up front.
    let net = InMemoryNetwork::new(N);
    assert!(matches!(
        eddsa::sign(
            &shares[0],
            &path,
            &message,
            &[0, 1],
            b"x",
            &net[0],
            Duration::from_secs(1)
        ),
        Err(eddsa::Error::Signers { t: 3 })
    ));

    // Encrypted storage round-trips and keeps the scheme apart.
    let doc = store::encrypt_eddsa_share(&shares[1], "correct horse", 1000).unwrap();
    let back = store::decrypt_eddsa_share(&doc, "correct horse").unwrap();
    assert_eq!(eddsa::vault_public_key(&back), pk);
    assert_eq!(back.index, 1);
    assert!(store::decrypt_eddsa_share(&doc, "wrong").is_err());
    assert!(store::decrypt_share(&doc, "correct horse").is_err());
    let dir = std::env::temp_dir().join(format!("keel-tss-eddsa-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("share-sol.enc");
    store::save_eddsa_share(&file, &shares[2], "pw").unwrap();
    let h = store::read_header(&file).unwrap();
    assert_eq!(h.scheme(), "eddsa");
    assert_eq!(h.index, 2);
    assert_eq!(h.public_key, hex::encode(pk));
    assert_eq!(
        eddsa::vault_public_key(&store::load_eddsa_share(&file, "pw").unwrap()),
        pk
    );
    let _ = std::fs::remove_dir_all(dir);
}
