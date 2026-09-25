//! Block payload codec: a bounded list of signed actions.

use crate::envelope::SignedAction;
use borsh::{BorshDeserialize, BorshSerialize};

pub const MAX_ACTIONS_PER_BLOCK: usize = 50_000;

#[derive(BorshSerialize, BorshDeserialize)]
struct Payload {
    actions: Vec<SignedAction>,
}

pub fn encode_payload(actions: &[SignedAction]) -> Vec<u8> {
    borsh::to_vec(&Payload {
        actions: actions.to_vec(),
    })
    .unwrap_or_default()
}

pub fn decode_payload(bytes: &[u8]) -> Option<Vec<SignedAction>> {
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    let p = Payload::try_from_slice(bytes).ok()?;
    if p.actions.len() > MAX_ACTIONS_PER_BLOCK {
        return None;
    }
    Some(p.actions)
}
