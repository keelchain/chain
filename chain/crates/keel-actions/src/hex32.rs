//! JSON representation of 32-byte hashes and keys: hex strings, with byte
//! arrays still accepted on input. Borsh is unaffected.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn parse(s: &str) -> Option<[u8; 32]> {
    let s = s.trim().trim_start_matches("0x");
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
    v.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .serialize(s)
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Hex(String),
        Bytes(Vec<u8>),
    }
    match Repr::deserialize(d)? {
        Repr::Hex(h) => parse(&h).ok_or_else(|| serde::de::Error::custom("expected 64 hex chars")),
        Repr::Bytes(b) => b
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 32 bytes")),
    }
}

pub mod option {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => super::serialize(b, s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        #[derive(Deserialize)]
        struct Wrap(#[serde(with = "super")] [u8; 32]);
        Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
    }
}
