//! BOLT11 invoices for the Keelchain (2026-09-10: Lightning deposits
//! and withdrawals). Pure parsing and hashing, deterministic, so the VM can
//! verify what an observer reports: the invoice's payee is the observer's
//! registered node, the payment hash matches the preimage, the amount and
//! the owner binding (`keel:<address>` in the description) are as claimed.
#![forbid(unsafe_code)]

use lightning_invoice::{Bolt11Invoice, Bolt11InvoiceDescriptionRef};
use sha2::{Digest, Sha256};
use std::str::FromStr;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LnError {
    #[error("invalid BOLT11 invoice: {0}")]
    Invalid(String),
    #[error("invoice has no amount")]
    NoAmount,
    #[error("invoice expired")]
    Expired,
}

/// What the chain needs from an invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    pub payment_hash: [u8; 32],
    /// Payee node id, compressed secp256k1 (33 bytes).
    pub payee: [u8; 33],
    pub amount_msat: u64,
    /// Human-readable prefix: "lnbc" (mainnet), "lntb" (testnet), "lnbcrt" (regtest).
    pub network: String,
    pub description: String,
    /// Unix seconds.
    pub expires_at: u64,
    pub created_at: u64,
}

pub fn parse(bolt11: &str) -> Result<Invoice, LnError> {
    let inv =
        Bolt11Invoice::from_str(bolt11.trim()).map_err(|e| LnError::Invalid(e.to_string()))?;
    let amount_msat = inv.amount_milli_satoshis().ok_or(LnError::NoAmount)?;
    let payee = inv.recover_payee_pub_key().serialize();
    let description = match inv.description() {
        Bolt11InvoiceDescriptionRef::Direct(d) => d.to_string(),
        Bolt11InvoiceDescriptionRef::Hash(h) => format!("hash:{}", hex(h.0.as_ref())),
    };
    let created_at = inv.duration_since_epoch().as_secs();
    let expires_at = created_at.saturating_add(inv.expiry_time().as_secs());
    let network = inv.currency().to_string();
    Ok(Invoice {
        payment_hash: *inv.payment_hash().as_ref(),
        payee,
        amount_msat,
        network,
        description,
        expires_at,
        created_at,
    })
}

/// `sha256(preimage) == payment_hash`.
pub fn preimage_matches(preimage: &[u8; 32], payment_hash: &[u8; 32]) -> bool {
    let d = Sha256::digest(preimage);
    d.as_slice() == payment_hash
}

/// The description an KEEL deposit invoice must carry: binds the invoice to
/// the chain account it credits.
pub fn deposit_description(owner_hex: &str) -> String {
    format!("keel:{owner_hex}")
}

pub fn owner_of_description(description: &str) -> Option<&str> {
    let rest = description.strip_prefix("keel:")?;
    (rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(rest)
}

/// Millisatoshis → satoshis, rounding DOWN (a payer never overpays).
pub fn msat_to_sat(msat: u64) -> u64 {
    msat / 1_000
}

pub fn looks_like_invoice(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("lnbc")
        || l.starts_with("lntb")
        || l.starts_with("lnbcrt")
        || l.starts_with("lnsb")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    use bitcoin::hashes::Hash as _;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};

    /// A regtest invoice signed here with a fixed key: the round trip the
    /// observer performs (build/sign) and the VM performs (parse/verify).
    fn sample(owner: &str) -> (String, [u8; 33], [u8; 32]) {
        let secp = Secp256k1::new();
        let sk = SecretKey::from_slice(&[0x11u8; 32]).unwrap();
        let payee = sk.public_key(&secp).serialize();
        let preimage = [7u8; 32];
        let hash: [u8; 32] = Sha256::digest(preimage).into();
        let inv = InvoiceBuilder::new(Currency::Regtest)
            .description(deposit_description(owner))
            .payment_hash(bitcoin::hashes::sha256::Hash::from_slice(&hash).unwrap())
            .payment_secret(PaymentSecret([3u8; 32]))
            .amount_milli_satoshis(2_500_000_000)
            .current_timestamp()
            .min_final_cltv_expiry_delta(18)
            .expiry_time(std::time::Duration::from_secs(3_600))
            .build_signed(|h| secp.sign_ecdsa_recoverable(h, &sk))
            .unwrap();
        (inv.to_string(), payee, preimage)
    }

    #[test]
    fn parses_a_signed_invoice() {
        let owner = "ab".repeat(32);
        let (bolt11, payee, preimage) = sample(&owner);
        let i = parse(&bolt11).unwrap();
        assert_eq!(i.amount_msat, 2_500_000_000);
        assert_eq!(i.network, "bcrt");
        assert_eq!(owner_of_description(&i.description), Some(owner.as_str()));
        assert_eq!(i.payee, payee);
        assert!(preimage_matches(&preimage, &i.payment_hash));
        assert_eq!(i.expires_at, i.created_at + 3_600);
        assert_eq!(msat_to_sat(i.amount_msat), 2_500_000);
        assert!(looks_like_invoice(&bolt11));
        assert!(!looks_like_invoice("bc1qxyz"));
    }

    #[test]
    fn preimage_and_owner_binding() {
        let preimage = [7u8; 32];
        let hash: [u8; 32] = Sha256::digest(preimage).into();
        assert!(preimage_matches(&preimage, &hash));
        assert!(!preimage_matches(&[8u8; 32], &hash));
        let owner = "ab".repeat(32);
        assert_eq!(
            owner_of_description(&deposit_description(&owner)),
            Some(owner.as_str())
        );
        assert_eq!(owner_of_description("1 cup coffee"), None);
        assert!(parse("lnbc1notaninvoice").is_err());
    }
}
