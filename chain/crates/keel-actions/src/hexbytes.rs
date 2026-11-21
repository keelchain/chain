//! JSON input for variable-length keys (`Vec<u8>`): a hex string is
//! accepted as well as a byte array, so a client can send a public key the
//! way it holds one. Output stays a byte array, and Borsh is unaffected.

use serde::{Deserialize, Deserializer};

fn parse(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Hex(String),
        Bytes(Vec<u8>),
    }
    match Repr::deserialize(d)? {
        Repr::Bytes(b) => Ok(b),
        Repr::Hex(s) => parse(&s).ok_or_else(|| serde::de::Error::custom("expected hex or bytes")),
    }
}

#[cfg(test)]
mod tests {
    #[derive(serde::Deserialize)]
    struct K {
        #[serde(deserialize_with = "super::deserialize")]
        k: Vec<u8>,
    }

    #[test]
    fn a_key_reads_from_hex_or_from_bytes() {
        let hex: K = serde_json::from_str(r#"{"k":"0x02ab10"}"#).unwrap();
        let plain: K = serde_json::from_str(r#"{"k":"02ab10"}"#).unwrap();
        let bytes: K = serde_json::from_str(r#"{"k":[2,171,16]}"#).unwrap();
        assert_eq!(hex.k, vec![2, 171, 16]);
        assert_eq!(plain.k, hex.k);
        assert_eq!(bytes.k, hex.k);
        assert!(serde_json::from_str::<K>(r#"{"k":"02a"}"#).is_err());
        assert!(serde_json::from_str::<K>(r#"{"k":"zz"}"#).is_err());
    }
}
