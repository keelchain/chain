use keel_types::Address;

/// What a block tells the VM about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockContext {
    pub height: u64,
    /// Milliseconds since the Unix epoch (BFT time from consensus).
    pub timestamp: u64,
    /// Consensus key of the proposer, as a chain address when known.
    pub proposer: Option<Address>,
}

impl BlockContext {
    pub fn seconds(&self) -> u64 {
        self.timestamp / 1000
    }
}
