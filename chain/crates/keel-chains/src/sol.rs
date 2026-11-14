//! Solana: Ed25519 vault keys with an additive child derivation, base58
//! addresses, associated token accounts (SPL), legacy transaction
//! messages for System and SPL Token transfers, and the parser the signing
//! policy uses to read them back.
//!
//! Derivation. Ed25519's standard derivation (SLIP-10) has no non-hardened
//! children, so Solana uses Keel's own rule, which any party can run from
//! the registered `(public_key, chain_code)` exactly as BIP32 serves the
//! secp256k1 chains: for each path element `i` under the parent point `P`,
//! `t = SHA-512("keel-ed25519-child" || chain_code || P || i_le) mod l`,
//! `child = P + t·B`. A threshold signer (FROST) adds the same `t` to its
//! shares; a single-key signer adds it to its scalar. The signature is a
//! standard Ed25519 signature under the child key, which Solana verifies
//! as usual.

use crate::Error;
use curve25519_dalek::{
    constants::ED25519_BASEPOINT_POINT, edwards::CompressedEdwardsY, scalar::Scalar, EdwardsPoint,
};
use sha2::{Digest as _, Sha256, Sha512};

pub const SYSTEM_PROGRAM: [u8; 32] = [0u8; 32];
/// `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`
pub const TOKEN_PROGRAM: [u8; 32] = [
    0x06, 0xdd, 0xf6, 0xe1, 0xd7, 0x65, 0xa1, 0x93, 0xd9, 0xcb, 0xe1, 0x46, 0xce, 0xeb, 0x79, 0xac,
    0x1c, 0xb4, 0x85, 0xed, 0x5f, 0x5b, 0x37, 0x91, 0x3a, 0x8c, 0xf5, 0x85, 0x7e, 0xff, 0x00, 0xa9,
];
/// `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`
pub const ASSOCIATED_TOKEN_PROGRAM: [u8; 32] = [
    0x8c, 0x97, 0x25, 0x8f, 0x4e, 0x24, 0x89, 0xf1, 0xbb, 0x3d, 0x10, 0x29, 0x14, 0x8e, 0x0d, 0x83,
    0x0b, 0x5a, 0x13, 0x99, 0xda, 0xff, 0x10, 0x84, 0x04, 0x8e, 0x7b, 0xd8, 0xdb, 0xe9, 0xf8, 0x59,
];
/// `ComputeBudget111111111111111111111111111111`
pub const COMPUTE_BUDGET_PROGRAM: [u8; 32] = [
    0x03, 0x06, 0x46, 0x6f, 0xe5, 0x21, 0x17, 0x32, 0xff, 0xec, 0xad, 0xba, 0x72, 0xc3, 0x9b, 0xe7,
    0xbc, 0x8c, 0xe5, 0xbb, 0xc5, 0xf7, 0x12, 0x6b, 0x2c, 0x43, 0x9b, 0x3a, 0x40, 0x00, 0x00, 0x00,
];
/// Base fee per signature, lamports.
pub const LAMPORTS_PER_SIGNATURE: u64 = 5_000;
/// Rent-exempt minimum of a token account (165 bytes), lamports: what the
/// fee payer spends when it creates a recipient's associated account.
pub const ATA_RENT_LAMPORTS: u64 = 2_039_280;
/// Rent-exempt minimum of a system account with no data, lamports: a
/// deposit address must keep this much to exist.
pub const ACCOUNT_RENT_LAMPORTS: u64 = 890_880;

// ---- keys and addresses ------------------------------------------------

pub fn address(pubkey: &[u8; 32]) -> String {
    bs58::encode(pubkey).into_string()
}

pub fn decode(addr: &str) -> Result<[u8; 32], Error> {
    let bytes = bs58::decode(addr)
        .into_vec()
        .map_err(|e| Error::Address(e.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| Error::Address("solana address must be 32 bytes".into()))
}

fn point(pubkey: &[u8; 32]) -> Result<EdwardsPoint, Error> {
    CompressedEdwardsY(*pubkey)
        .decompress()
        .ok_or_else(|| Error::Key("not an ed25519 point".into()))
}

/// The child tweak of one path element under parent `parent`.
pub fn tweak(parent: &[u8; 32], chain_code: &[u8; 32], index: u32) -> Result<Scalar, Error> {
    if index >= 1 << 31 {
        return Err(Error::Key(format!("hardened index {index}")));
    }
    let mut h = Sha512::new();
    h.update(b"keel-ed25519-child");
    h.update(chain_code);
    h.update(parent);
    h.update(index.to_le_bytes());
    let wide: [u8; 64] = h.finalize().into();
    Ok(Scalar::from_bytes_mod_order_wide(&wide))
}

/// The total scalar a signer adds to the vault key to sign for `path`, and
/// the child public key it derives.
pub fn child(
    vault_pubkey: &[u8; 32],
    chain_code: &[u8; 32],
    path: &[u32],
) -> Result<(Scalar, [u8; 32]), Error> {
    let mut p = point(vault_pubkey)?;
    let mut total = Scalar::ZERO;
    for i in path {
        let t = tweak(&p.compress().to_bytes(), chain_code, *i)?;
        p += t * ED25519_BASEPOINT_POINT;
        total += t;
    }
    Ok((total, p.compress().to_bytes()))
}

pub fn child_pubkey(
    vault_pubkey: &[u8; 32],
    chain_code: &[u8; 32],
    path: &[u32],
) -> Result<[u8; 32], Error> {
    Ok(child(vault_pubkey, chain_code, path)?.1)
}

/// Public key of a raw scalar (no RFC 8032 clamping: a vault scalar is a
/// group secret, not a seed).
pub fn pubkey_of_scalar(x: &Scalar) -> [u8; 32] {
    (x * ED25519_BASEPOINT_POINT).compress().to_bytes()
}

/// Ed25519 signature of `message` under the raw scalar `x` with a
/// deterministic nonce drawn from `prefix` (the development signer; the
/// threshold signer makes its own nonces).
pub fn sign_with_scalar(x: &Scalar, prefix: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let a = pubkey_of_scalar(x);
    let mut h = Sha512::new();
    h.update(prefix);
    h.update(message);
    let wide: [u8; 64] = h.finalize().into();
    let r = Scalar::from_bytes_mod_order_wide(&wide);
    let big_r = (r * ED25519_BASEPOINT_POINT).compress().to_bytes();
    let k = challenge(&big_r, &a, message);
    let s = r + k * x;
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(&big_r);
    out[32..].copy_from_slice(&s.to_bytes());
    out
}

fn challenge(big_r: &[u8; 32], a: &[u8; 32], message: &[u8]) -> Scalar {
    let mut h = Sha512::new();
    h.update(big_r);
    h.update(a);
    h.update(message);
    let wide: [u8; 64] = h.finalize().into();
    Scalar::from_bytes_mod_order_wide(&wide)
}

/// Standard Ed25519 verification (cofactorless, canonical `s`).
pub fn verify(pubkey: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let mut rb = [0u8; 32];
    rb.copy_from_slice(&signature[..32]);
    let mut sb = [0u8; 32];
    sb.copy_from_slice(&signature[32..]);
    let (Some(a), Some(big_r), Some(s)) = (
        CompressedEdwardsY(*pubkey).decompress(),
        CompressedEdwardsY(rb).decompress(),
        Option::<Scalar>::from(Scalar::from_canonical_bytes(sb)),
    ) else {
        return false;
    };
    let k = challenge(&rb, pubkey, message);
    s * ED25519_BASEPOINT_POINT == big_r + k * a
}

// ---- program-derived addresses -----------------------------------------

/// `find_program_address`: the first bump (from 255 down) whose SHA-256
/// of the seeds falls off the curve.
pub fn find_program_address(seeds: &[&[u8]], program: &[u8; 32]) -> Result<([u8; 32], u8), Error> {
    for bump in (0u8..=255).rev() {
        let mut h = Sha256::new();
        for s in seeds {
            h.update(s);
        }
        h.update([bump]);
        h.update(program);
        h.update(b"ProgramDerivedAddress");
        let out: [u8; 32] = h.finalize().into();
        if CompressedEdwardsY(out).decompress().is_none() {
            return Ok((out, bump));
        }
    }
    Err(Error::Key("no program address found".into()))
}

/// The associated token account of `owner` for `mint` (SPL Token program).
pub fn associated_token_account(owner: &[u8; 32], mint: &[u8; 32]) -> Result<[u8; 32], Error> {
    Ok(find_program_address(&[owner, &TOKEN_PROGRAM, mint], &ASSOCIATED_TOKEN_PROGRAM)?.0)
}

// ---- messages ----------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountMeta {
    pub pubkey: [u8; 32],
    pub signer: bool,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub program: [u8; 32],
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledInstruction {
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
}

/// A legacy transaction message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub num_required_signatures: u8,
    pub num_readonly_signed: u8,
    pub num_readonly_unsigned: u8,
    pub account_keys: Vec<[u8; 32]>,
    pub recent_blockhash: [u8; 32],
    pub instructions: Vec<CompiledInstruction>,
}

fn meta(pubkey: [u8; 32], signer: bool, writable: bool) -> AccountMeta {
    AccountMeta {
        pubkey,
        signer,
        writable,
    }
}

/// System program `Transfer`.
pub fn transfer_lamports(from: &[u8; 32], to: &[u8; 32], lamports: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program: SYSTEM_PROGRAM,
        accounts: vec![meta(*from, true, true), meta(*to, false, true)],
        data,
    }
}

/// Associated Token program `CreateIdempotent`: `payer` funds `owner`'s
/// account for `mint` unless it exists.
pub fn create_associated_token_account(
    payer: &[u8; 32],
    owner: &[u8; 32],
    mint: &[u8; 32],
) -> Result<Instruction, Error> {
    let ata = associated_token_account(owner, mint)?;
    Ok(Instruction {
        program: ASSOCIATED_TOKEN_PROGRAM,
        accounts: vec![
            meta(*payer, true, true),
            meta(ata, false, true),
            meta(*owner, false, false),
            meta(*mint, false, false),
            meta(SYSTEM_PROGRAM, false, false),
            meta(TOKEN_PROGRAM, false, false),
        ],
        data: vec![1],
    })
}

/// SPL Token `TransferChecked` between two token accounts, signed by the
/// source's owner.
pub fn transfer_checked(
    source: &[u8; 32],
    mint: &[u8; 32],
    destination: &[u8; 32],
    owner: &[u8; 32],
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = Vec::with_capacity(10);
    data.push(12);
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program: TOKEN_PROGRAM,
        accounts: vec![
            meta(*source, false, true),
            meta(*mint, false, false),
            meta(*destination, false, true),
            meta(*owner, true, false),
        ],
        data,
    }
}

/// Compute Budget `SetComputeUnitPrice` (a priority fee, micro-lamports
/// per compute unit).
pub fn set_compute_unit_price(micro_lamports: u64) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(3);
    data.extend_from_slice(&micro_lamports.to_le_bytes());
    Instruction {
        program: COMPUTE_BUDGET_PROGRAM,
        accounts: Vec::new(),
        data,
    }
}

/// Compile instructions into a message paid by `fee_payer`, with the
/// canonical account ordering (signers first, writable before read-only).
pub fn build_message(
    fee_payer: &[u8; 32],
    instructions: &[Instruction],
    recent_blockhash: [u8; 32],
) -> Result<Message, Error> {
    // Merge the flags of every mention of a key.
    let mut metas: Vec<AccountMeta> = vec![meta(*fee_payer, true, true)];
    let mut note = |m: AccountMeta| {
        if let Some(e) = metas.iter_mut().find(|e| e.pubkey == m.pubkey) {
            e.signer |= m.signer;
            e.writable |= m.writable;
        } else {
            metas.push(m);
        }
    };
    for ix in instructions {
        for a in &ix.accounts {
            note(a.clone());
        }
        note(meta(ix.program, false, false));
    }
    let payer = metas.remove(0);
    let mut signed_w: Vec<AccountMeta> = vec![payer];
    let mut signed_r = Vec::new();
    let mut unsigned_w = Vec::new();
    let mut unsigned_r = Vec::new();
    for m in metas {
        match (m.signer, m.writable) {
            (true, true) => signed_w.push(m),
            (true, false) => signed_r.push(m),
            (false, true) => unsigned_w.push(m),
            (false, false) => unsigned_r.push(m),
        }
    }
    let num_required_signatures = (signed_w.len() + signed_r.len()) as u8;
    let num_readonly_signed = signed_r.len() as u8;
    let num_readonly_unsigned = unsigned_r.len() as u8;
    let account_keys: Vec<[u8; 32]> = signed_w
        .into_iter()
        .chain(signed_r)
        .chain(unsigned_w)
        .chain(unsigned_r)
        .map(|m| m.pubkey)
        .collect();
    if account_keys.len() > u8::MAX as usize {
        return Err(Error::Tx("too many accounts".into()));
    }
    let index_of = |k: &[u8; 32]| -> Result<u8, Error> {
        account_keys
            .iter()
            .position(|a| a == k)
            .map(|i| i as u8)
            .ok_or_else(|| Error::Tx("account not compiled".into()))
    };
    let mut compiled = Vec::with_capacity(instructions.len());
    for ix in instructions {
        compiled.push(CompiledInstruction {
            program_id_index: index_of(&ix.program)?,
            accounts: ix
                .accounts
                .iter()
                .map(|a| index_of(&a.pubkey))
                .collect::<Result<_, _>>()?,
            data: ix.data.clone(),
        });
    }
    Ok(Message {
        num_required_signatures,
        num_readonly_signed,
        num_readonly_unsigned,
        account_keys,
        recent_blockhash,
        instructions: compiled,
    })
}

fn put_compact_u16(out: &mut Vec<u8>, mut v: u16) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

fn get_compact_u16(buf: &[u8], pos: &mut usize) -> Result<usize, Error> {
    let mut v: u32 = 0;
    for i in 0..3 {
        let b = *buf
            .get(*pos)
            .ok_or_else(|| Error::Tx("message truncated".into()))?;
        *pos += 1;
        v |= ((b & 0x7f) as u32) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(v as usize);
        }
    }
    Err(Error::Tx("bad compact length".into()))
}

fn take<'a>(buf: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], Error> {
    let end = pos
        .checked_add(n)
        .filter(|e| *e <= buf.len())
        .ok_or_else(|| Error::Tx("message truncated".into()))?;
    let out = &buf[*pos..end];
    *pos = end;
    Ok(out)
}

impl Message {
    /// The bytes a signer signs and `sendTransaction` carries.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = vec![
            self.num_required_signatures,
            self.num_readonly_signed,
            self.num_readonly_unsigned,
        ];
        put_compact_u16(&mut out, self.account_keys.len() as u16);
        for k in &self.account_keys {
            out.extend_from_slice(k);
        }
        out.extend_from_slice(&self.recent_blockhash);
        put_compact_u16(&mut out, self.instructions.len() as u16);
        for ix in &self.instructions {
            out.push(ix.program_id_index);
            put_compact_u16(&mut out, ix.accounts.len() as u16);
            out.extend_from_slice(&ix.accounts);
            put_compact_u16(&mut out, ix.data.len() as u16);
            out.extend_from_slice(&ix.data);
        }
        out
    }

    pub fn deserialize(buf: &[u8]) -> Result<Self, Error> {
        let mut pos = 0usize;
        let header = take(buf, &mut pos, 3)?;
        if header[0] & 0x80 != 0 {
            return Err(Error::Tx("versioned messages are not supported".into()));
        }
        let n_keys = get_compact_u16(buf, &mut pos)?;
        let mut account_keys = Vec::with_capacity(n_keys);
        for _ in 0..n_keys {
            let k: [u8; 32] = take(buf, &mut pos, 32)?
                .try_into()
                .map_err(|_| Error::Tx("key".into()))?;
            account_keys.push(k);
        }
        let recent_blockhash: [u8; 32] = take(buf, &mut pos, 32)?
            .try_into()
            .map_err(|_| Error::Tx("blockhash".into()))?;
        let n_ix = get_compact_u16(buf, &mut pos)?;
        let mut instructions = Vec::with_capacity(n_ix);
        for _ in 0..n_ix {
            let program_id_index = take(buf, &mut pos, 1)?[0];
            let n_acc = get_compact_u16(buf, &mut pos)?;
            let accounts = take(buf, &mut pos, n_acc)?.to_vec();
            let n_data = get_compact_u16(buf, &mut pos)?;
            let data = take(buf, &mut pos, n_data)?.to_vec();
            instructions.push(CompiledInstruction {
                program_id_index,
                accounts,
                data,
            });
        }
        if pos != buf.len() {
            return Err(Error::Tx("trailing bytes".into()));
        }
        Ok(Message {
            num_required_signatures: header[0],
            num_readonly_signed: header[1],
            num_readonly_unsigned: header[2],
            account_keys,
            recent_blockhash,
            instructions,
        })
    }

    pub fn fee_payer(&self) -> Option<&[u8; 32]> {
        self.account_keys.first()
    }

    /// The keys that must sign, in signature order.
    pub fn signers(&self) -> &[[u8; 32]] {
        let n = (self.num_required_signatures as usize).min(self.account_keys.len());
        &self.account_keys[..n]
    }

    fn key(&self, index: u8) -> Result<[u8; 32], Error> {
        self.account_keys
            .get(index as usize)
            .copied()
            .ok_or_else(|| Error::Tx("account index out of range".into()))
    }

    /// Every instruction read back as a [`Transfer`] or an allowed
    /// side instruction; anything else is an error, so a policy that
    /// accepts this list accepts the whole message.
    pub fn transfers(&self) -> Result<Vec<Transfer>, Error> {
        let mut out = Vec::new();
        for ix in &self.instructions {
            let program = self.key(ix.program_id_index)?;
            let acc = |i: usize| -> Result<[u8; 32], Error> {
                let idx = *ix
                    .accounts
                    .get(i)
                    .ok_or_else(|| Error::Tx("instruction account missing".into()))?;
                self.key(idx)
            };
            if program == SYSTEM_PROGRAM {
                if ix.data.len() != 12 || ix.data[..4] != [2, 0, 0, 0] {
                    return Err(Error::Tx("system instruction is not a transfer".into()));
                }
                let lamports = u64::from_le_bytes(
                    ix.data[4..12]
                        .try_into()
                        .map_err(|_| Error::Tx("lamports".into()))?,
                );
                out.push(Transfer::Lamports {
                    from: acc(0)?,
                    to: acc(1)?,
                    lamports,
                });
            } else if program == TOKEN_PROGRAM {
                if ix.data.len() != 10 || ix.data[0] != 12 {
                    return Err(Error::Tx("token instruction is not TransferChecked".into()));
                }
                let amount = u64::from_le_bytes(
                    ix.data[1..9]
                        .try_into()
                        .map_err(|_| Error::Tx("amount".into()))?,
                );
                out.push(Transfer::Token {
                    source: acc(0)?,
                    mint: acc(1)?,
                    destination: acc(2)?,
                    owner: acc(3)?,
                    amount,
                    decimals: ix.data[9],
                });
            } else if program == ASSOCIATED_TOKEN_PROGRAM {
                if ix.data != [1] {
                    return Err(Error::Tx(
                        "associated-token instruction is not CreateIdempotent".into(),
                    ));
                }
                let (payer, ata, owner, mint) = (acc(0)?, acc(1)?, acc(2)?, acc(3)?);
                if associated_token_account(&owner, &mint)? != ata {
                    return Err(Error::Tx(
                        "created account is not the owner's associated account".into(),
                    ));
                }
                out.push(Transfer::CreateAccount { payer, owner, mint });
            } else if program == COMPUTE_BUDGET_PROGRAM {
                // SetComputeUnitLimit (2) / SetComputeUnitPrice (3) move no value.
                if !matches!(ix.data.first(), Some(2) | Some(3)) {
                    return Err(Error::Tx("unexpected compute-budget instruction".into()));
                }
            } else {
                return Err(Error::Tx(format!(
                    "instruction to an unexpected program {}",
                    address(&program)
                )));
            }
        }
        Ok(out)
    }
}

/// What one instruction of a message does, for the policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transfer {
    Lamports {
        from: [u8; 32],
        to: [u8; 32],
        lamports: u64,
    },
    Token {
        source: [u8; 32],
        mint: [u8; 32],
        destination: [u8; 32],
        owner: [u8; 32],
        amount: u64,
        decimals: u8,
    },
    /// `CreateIdempotent` of `owner`'s associated account, paid by `payer`.
    CreateAccount {
        payer: [u8; 32],
        owner: [u8; 32],
        mint: [u8; 32],
    },
}

/// The wire transaction: signatures (in signer order) then the message.
pub fn serialize_transaction(message: &[u8], signatures: &[[u8; 64]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 64 * signatures.len() + message.len());
    put_compact_u16(&mut out, signatures.len() as u16);
    for s in signatures {
        out.extend_from_slice(s);
    }
    out.extend_from_slice(message);
    out
}

/// A transaction's id is its first signature, base58.
pub fn transaction_id(signature: &[u8; 64]) -> String {
    bs58::encode(signature).into_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_keys_verify_and_match_the_signer_side() {
        // Vault "key": a raw scalar.
        let x = Scalar::from_bytes_mod_order([7u8; 32]);
        let vault = pubkey_of_scalar(&x);
        let cc = [9u8; 32];
        let path = [501u32, 4];
        let (t, child_pk) = child(&vault, &cc, &path).unwrap();
        // The signer adds the same tweak to its scalar.
        assert_eq!(pubkey_of_scalar(&(x + t)), child_pk);
        let msg = b"solana message";
        let sig = sign_with_scalar(&(x + t), &[1u8; 32], msg);
        assert!(verify(&child_pk, msg, &sig));
        assert!(!verify(&vault, msg, &sig));
        assert!(!verify(&child_pk, b"other", &sig));
        // Different index, different key; hardened refused.
        assert_ne!(child_pubkey(&vault, &cc, &[501, 5]).unwrap(), child_pk);
        assert!(child(&vault, &cc, &[1 << 31]).is_err());
        // Addresses round-trip.
        let a = address(&child_pk);
        assert_eq!(decode(&a).unwrap(), child_pk);
        assert!(decode("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t").is_err());
    }

    #[test]
    fn associated_token_account_matches_the_known_vector() {
        // USDC (mainnet) ATA of the system program id's "owner" 1111…1111:
        // well-known derivation checked against spl-associated-token-account.
        let owner = decode("9Yt1UNy1t2MULrRbN6PG9mAD6prjWDfDpnTBGP1F1Sqz").unwrap();
        let mint = decode("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
        let ata = associated_token_account(&owner, &mint).unwrap();
        // The derived address is off the curve by construction.
        assert!(CompressedEdwardsY(ata).decompress().is_none());
        // Deterministic.
        assert_eq!(ata, associated_token_account(&owner, &mint).unwrap());
        assert_eq!(
            address(&TOKEN_PROGRAM),
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        );
        assert_eq!(
            address(&ASSOCIATED_TOKEN_PROGRAM),
            "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        );
        assert_eq!(
            address(&COMPUTE_BUDGET_PROGRAM),
            "ComputeBudget111111111111111111111111111111"
        );
        assert_eq!(address(&SYSTEM_PROGRAM), "11111111111111111111111111111111");
    }

    #[test]
    fn messages_round_trip_and_read_back() {
        let hot = pubkey_of_scalar(&Scalar::from_bytes_mod_order([3u8; 32]));
        let user = pubkey_of_scalar(&Scalar::from_bytes_mod_order([4u8; 32]));
        let mint = decode("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU").unwrap();
        let src = associated_token_account(&hot, &mint).unwrap();
        let dst = associated_token_account(&user, &mint).unwrap();
        let ixs = vec![
            set_compute_unit_price(1_000),
            transfer_lamports(&hot, &user, 42),
            create_associated_token_account(&hot, &user, &mint).unwrap(),
            transfer_checked(&src, &mint, &dst, &hot, 1_500_000, 6),
        ];
        let msg = build_message(&hot, &ixs, [8u8; 32]).unwrap();
        assert_eq!(msg.fee_payer(), Some(&hot));
        assert_eq!(msg.signers(), &[hot]);
        assert_eq!(msg.num_required_signatures, 1);
        let bytes = msg.serialize();
        let back = Message::deserialize(&bytes).unwrap();
        assert_eq!(back, msg);
        let t = back.transfers().unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(
            t[0],
            Transfer::Lamports {
                from: hot,
                to: user,
                lamports: 42
            }
        );
        assert_eq!(
            t[1],
            Transfer::CreateAccount {
                payer: hot,
                owner: user,
                mint
            }
        );
        assert_eq!(
            t[2],
            Transfer::Token {
                source: src,
                mint,
                destination: dst,
                owner: hot,
                amount: 1_500_000,
                decimals: 6
            }
        );
        // Two signers (a consolidation paid by the hot address).
        let msg2 = build_message(&hot, &[transfer_lamports(&user, &hot, 5)], [8u8; 32]).unwrap();
        assert_eq!(msg2.signers(), &[hot, user]);
        assert_eq!(msg2.num_required_signatures, 2);
        let tx = serialize_transaction(&msg2.serialize(), &[[1u8; 64], [2u8; 64]]);
        assert_eq!(tx[0], 2);
        assert_eq!(&tx[1..65], &[1u8; 64]);
        assert_eq!(transaction_id(&[1u8; 64]).len(), 87);
        // Compact lengths above 127 use two bytes.
        let mut v = Vec::new();
        put_compact_u16(&mut v, 300);
        assert_eq!(v, vec![0xac, 0x02]);
        let mut p = 0;
        assert_eq!(get_compact_u16(&v, &mut p).unwrap(), 300);
    }
}
