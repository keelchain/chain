use crate::action::Action;
use borsh::{BorshDeserialize, BorshSerialize};
use keel_crypto::{sha256, Hash32, Keypair};
use keel_types::Address;
use serde::{Deserialize, Serialize};

pub const CHAIN_ID_DEVNET: u32 = 1;
const DOMAIN: &[u8] = b"keel-action-v1";

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Envelope {
    pub signer: Address,
    /// Strictly increasing per signer, starting at 0.
    pub nonce: u64,
    pub chain_id: u32,
    pub action: Action,
}

impl Envelope {
    pub fn digest(&self) -> Hash32 {
        let bytes = borsh::to_vec(self).unwrap_or_default();
        sha256(&[DOMAIN, &bytes])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct SignedAction {
    pub envelope: Envelope,
    #[serde(with = "sig64")]
    pub signature: [u8; 64],
}

mod sig64 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        hex_str(v).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let s = String::deserialize(d)?;
        let bytes = from_hex(&s).ok_or_else(|| serde::de::Error::custom("bad hex"))?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))
    }

    fn hex_str(v: &[u8]) -> String {
        v.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn from_hex(s: &str) -> Option<Vec<u8>> {
        if !s.len().is_multiple_of(2) {
            return None;
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
            .collect()
    }
}

impl SignedAction {
    pub fn sign(keypair: &Keypair, nonce: u64, chain_id: u32, action: Action) -> Self {
        let envelope = Envelope {
            signer: keypair.address(),
            nonce,
            chain_id,
            action,
        };
        let signature = keypair.sign(&envelope.digest());
        Self {
            envelope,
            signature,
        }
    }

    pub fn verify(&self) -> bool {
        keel_crypto::verify(
            &self.envelope.signer,
            &self.envelope.digest(),
            &self.signature,
        )
    }

    /// Transaction id: hash of the signed bytes.
    pub fn id(&self) -> Hash32 {
        sha256(&[b"keel-txid", &self.envelope.digest(), &self.signature])
    }

    pub fn signer(&self) -> Address {
        self.envelope.signer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Transfer;
    use keel_types::Asset;

    #[test]
    fn signature_binds_every_field() {
        let k = Keypair::from_seed(1);
        let a = Action::Transfer(Transfer {
            to: Address::tagged(2),
            asset: Asset::new("KEEL"),
            amount: 5,
            memo: None,
        });
        let s = SignedAction::sign(&k, 0, CHAIN_ID_DEVNET, a.clone());
        assert!(s.verify());
        let mut t = s.clone();
        t.envelope.nonce = 1;
        assert!(!t.verify());
        let mut t = s.clone();
        t.envelope.chain_id = 2;
        assert!(!t.verify());
        let mut t = s.clone();
        t.envelope.signer = Address::tagged(9);
        assert!(!t.verify());
        assert_ne!(s.id(), SignedAction::sign(&k, 1, CHAIN_ID_DEVNET, a).id());
    }
}
