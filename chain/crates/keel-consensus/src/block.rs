//! The block consensus orders. Its payload is opaque here: the VM defines
//! the action codec, this crate only carries bytes and commits to them in
//! the digest.

use crate::types::{Context, Digest, Hasher};
use bytes::{Buf, BufMut, Bytes};
use commonware_codec::{varint::UInt, Encode, EncodeSize, Error, Read, ReadExt, Write};
use commonware_consensus::{types::Height, CertifiableBlock, Heightable};
use commonware_cryptography::{Digestible, Hasher as _};

/// Hard cap on a block's payload. Governance may lower the effective cap;
/// the codec refuses anything above this.
pub const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The consensus context when this block was proposed.
    pub context: Context,
    /// The parent block's digest.
    pub parent: Digest,
    pub height: Height,
    /// Milliseconds since the Unix epoch, strictly increasing along the chain.
    pub timestamp: u64,
    /// Encoded actions (see `keel-actions`); empty for an idle block.
    pub payload: Bytes,
    digest: Digest,
}

impl Block {
    fn compute_digest(
        context: &Context,
        parent: &Digest,
        height: Height,
        timestamp: u64,
        payload: &[u8],
    ) -> Digest {
        let mut hasher = Hasher::default();
        hasher.update(&context.encode());
        hasher.update(parent.as_ref());
        hasher.update(&height.get().to_be_bytes());
        hasher.update(&timestamp.to_be_bytes());
        hasher.update(payload);
        hasher.finalize().1
    }

    pub fn new(
        context: Context,
        parent: Digest,
        height: Height,
        timestamp: u64,
        payload: Bytes,
    ) -> Self {
        let digest = Self::compute_digest(&context, &parent, height, timestamp, &payload);
        Self {
            context,
            parent,
            height,
            timestamp,
            payload,
            digest,
        }
    }
}

impl Write for Block {
    fn write(&self, writer: &mut impl BufMut) {
        self.context.write(writer);
        self.parent.write(writer);
        self.height.write(writer);
        UInt(self.timestamp).write(writer);
        self.payload.write(writer);
    }
}

impl Read for Block {
    type Cfg = ();

    fn read_cfg(reader: &mut impl Buf, _: &Self::Cfg) -> Result<Self, Error> {
        let context = Context::read(reader)?;
        let parent = Digest::read(reader)?;
        let height = Height::read(reader)?;
        let timestamp = UInt::read(reader)?.into();
        let payload = Bytes::read_cfg(reader, &(..=MAX_PAYLOAD_BYTES).into())?;
        Ok(Self::new(context, parent, height, timestamp, payload))
    }
}

impl EncodeSize for Block {
    fn encode_size(&self) -> usize {
        self.context.encode_size()
            + self.parent.encode_size()
            + self.height.encode_size()
            + UInt(self.timestamp).encode_size()
            + self.payload.encode_size()
    }
}

impl Digestible for Block {
    type Digest = Digest;

    fn digest(&self) -> Digest {
        self.digest
    }
}

impl commonware_consensus::Block for Block {
    fn parent(&self) -> Digest {
        self.parent
    }
}

impl Heightable for Block {
    fn height(&self) -> Height {
        self.height
    }
}

impl CertifiableBlock for Block {
    type Context = Context;

    fn context(&self) -> Self::Context {
        self.context.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PrivateKey, EPOCH};
    use commonware_codec::DecodeExt;
    use commonware_consensus::types::{Round, View};
    use commonware_cryptography::{Digest as _, Signer};

    fn ctx() -> Context {
        Context {
            round: Round::new(EPOCH, View::new(3)),
            leader: PrivateKey::from_seed(1).public_key(),
            parent: (View::new(2), Digest::EMPTY),
        }
    }

    #[test]
    fn codec_round_trip_preserves_digest() {
        let block = Block::new(
            ctx(),
            Digest::EMPTY,
            Height::new(7),
            1_700_000_000_000,
            Bytes::from_static(b"actions"),
        );
        let decoded = Block::decode(block.encode()).unwrap();
        assert_eq!(decoded, block);
        assert_eq!(decoded.digest(), block.digest());
        // Payload is committed to.
        let other = Block::new(
            ctx(),
            Digest::EMPTY,
            Height::new(7),
            1_700_000_000_000,
            Bytes::from_static(b"actionz"),
        );
        assert_ne!(other.digest(), block.digest());
    }

    #[test]
    fn oversized_payload_is_refused() {
        let block = Block::new(
            ctx(),
            Digest::EMPTY,
            Height::new(1),
            1,
            Bytes::from(vec![0u8; MAX_PAYLOAD_BYTES + 1]),
        );
        assert!(Block::decode(block.encode()).is_err());
    }
}
