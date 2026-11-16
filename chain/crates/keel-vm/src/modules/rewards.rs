//! What the chain mints after genesis, and the only thing it mints: the
//! block reward for validators and the community reward for the users whose
//! fees keep the chain busy. Both schedules are constants here, not
//! parameters: changing them is a software upgrade validators choose to
//! run, never a vote. Both are capped, so total supply converges to
//! [`KEEL_CAP`] and never passes it (decided 2026-11-16).
//!
//! **Block reward.** Every block mints KEEL for the time elapsed since the
//! previous block at a yearly rate that starts at one eighth of the
//! validator issuance (so the first four years mint half of it) and halves
//! every four years: Bitcoin's schedule, by time rather than block count
//! because block intervals vary. A slice ([`PROPOSER_BPS`]) goes straight to
//! the block's proposer, so producing blocks earns more than holding stake;
//! the rest joins `validator_rewards` and is paid at the epoch boundary pro
//! rata to bonded power, observers' share first, delegators included.
//!
//! **Community reward.** Every protocol fee a user pays in a vault asset or
//! the dollar (not KEEL usage paid by clients) mints [`COMMUNITY_KEEL_PER_USD`]
//! per dollar of fee to the payer, until the community issuance is spent.
//! Issuance therefore tracks usage: a quiet chain mints nothing, a busy one
//! rewards the people making it busy.

use crate::{context::BlockContext, modules::tokens, receipt::Event, state::State};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_ledger::{Record, TxType};
use keel_types::{mul_div_floor, Address, Amount};
use serde::{Deserialize, Serialize};

/// 21 billion KEEL, six decimals.
pub const KEEL_CAP: Amount = 21_000_000_000 * 1_000_000;
/// Four years in milliseconds: the halving interval.
pub const HALVING_MS: u64 = 4 * 365 * 86_400_000;
const YEAR_MS: u128 = 365 * 86_400_000;
/// Share of each block's mint paid to its proposer; the rest to the pool.
pub const PROPOSER_BPS: u32 = 1_000;
/// KEEL minted to a fee payer per dollar of protocol fee.
pub const COMMUNITY_KEEL_PER_USD: Amount = 10 * 1_000_000;

#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct RewardsState {
    /// Most KEEL the block reward may ever mint (10% of the cap by default).
    pub validator_cap: Amount,
    pub validator_minted: Amount,
    /// Most KEEL the community reward may ever mint (25% of the cap less the
    /// awards issued at genesis).
    pub community_cap: Amount,
    pub community_minted: Amount,
    /// Block time (ms) of the first block: the schedule's origin. 0 = not
    /// started.
    pub start_ms: u64,
    /// Block time (ms) the reward was last minted to.
    pub last_mint_ms: u64,
    /// Community rewards minted during the current block, emitted as
    /// end-of-block events.
    pub pending: Vec<Event>,
}

impl RewardsState {
    pub fn validator_remaining(&self) -> Amount {
        self.validator_cap.saturating_sub(self.validator_minted)
    }
    pub fn community_remaining(&self) -> Amount {
        self.community_cap.saturating_sub(self.community_minted)
    }
}

/// Yearly block-reward rate at `at_ms`: the first-era rate halved once per
/// four years since the schedule began.
pub fn rate_per_year(state: &State, at_ms: u64) -> Amount {
    if state.rewards.start_ms == 0 || state.rewards.validator_cap == 0 {
        return 0;
    }
    let eras = at_ms.saturating_sub(state.rewards.start_ms) / HALVING_MS;
    let first = state.rewards.validator_cap / 8;
    first >> eras.min(120)
}

/// When the next halving happens, in block time (ms).
pub fn next_halving_ms(state: &State, at_ms: u64) -> Option<u64> {
    if state.rewards.start_ms == 0 {
        return None;
    }
    let eras = at_ms.saturating_sub(state.rewards.start_ms) / HALVING_MS;
    Some(state.rewards.start_ms + (eras + 1) * HALVING_MS)
}

/// The validator address behind a block's proposer key, if it is one.
fn proposer_address(state: &State, key: Address) -> Option<Address> {
    state
        .staking
        .validators
        .iter()
        .find(|(_, v)| v.consensus_key == key.0)
        .map(|(a, _)| *a)
}

/// Mint this block's reward. Called at the end of every block, before the
/// epoch distribution, so an epoch pays what its blocks earned.
pub fn mint_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    if state.rewards.start_ms == 0 {
        state.rewards.start_ms = ctx.timestamp;
        state.rewards.last_mint_ms = ctx.timestamp;
        return Vec::new();
    }
    let elapsed = ctx.timestamp.saturating_sub(state.rewards.last_mint_ms) as u128;
    state.rewards.last_mint_ms = ctx.timestamp;
    let rate = rate_per_year(state, ctx.timestamp);
    let amount = (rate.saturating_mul(elapsed) / YEAR_MS).min(state.rewards.validator_remaining());
    if amount == 0 {
        return Vec::new();
    }
    let proposer = ctx.proposer.and_then(|k| proposer_address(state, k));
    let to_proposer = match proposer {
        Some(_) => mul_div_floor(amount, PROPOSER_BPS as Amount, 10_000).unwrap_or(0),
        None => 0,
    };
    let native = state.tokens.native.clone();
    let mut records = vec![Record::debit(
        tokens::system_key(&native, "issuance"),
        amount,
    )];
    if amount - to_proposer > 0 {
        records.push(Record::credit(
            tokens::system_key(&native, "validator_rewards"),
            amount - to_proposer,
        ));
    }
    if let (Some(p), true) = (proposer, to_proposer > 0) {
        records.push(Record::credit(tokens::deposit_key(p, &native), to_proposer));
    }
    if state
        .ledger
        .post(
            &format!("block-reward:{}", ctx.height),
            TxType::BlockReward,
            None,
            None,
            records,
        )
        .is_err()
    {
        return Vec::new();
    }
    state.rewards.validator_minted = state.rewards.validator_minted.saturating_add(amount);
    vec![Event::BlockRewardMinted {
        height: ctx.height,
        amount,
        proposer,
        to_proposer,
    }]
}

/// A user paid `fee` of `asset` as a protocol fee: mint its community
/// reward. Nothing for KEEL (client usage) or for system payers.
pub fn community_for_fee(
    state: &mut State,
    payer: Address,
    asset: &keel_types::Asset,
    fee: Amount,
    external_id: &str,
) {
    if payer.is_system() || *asset == state.tokens.native {
        return;
    }
    let Some(usd) = tokens::usd_value(state, asset, fee) else {
        return;
    };
    let amount = mul_div_floor(usd, COMMUNITY_KEEL_PER_USD, 1_000_000)
        .unwrap_or(0)
        .min(state.rewards.community_remaining());
    if amount == 0 {
        return;
    }
    let native = state.tokens.native.clone();
    if state
        .ledger
        .post(
            &format!("community:{external_id}"),
            TxType::CommunityReward,
            None,
            None,
            vec![
                Record::debit(tokens::system_key(&native, "issuance"), amount),
                Record::credit(tokens::deposit_key(payer, &native), amount),
            ],
        )
        .is_err()
    {
        return;
    }
    state.rewards.community_minted = state.rewards.community_minted.saturating_add(amount);
    state.rewards.pending.push(Event::CommunityRewardMinted {
        owner: payer,
        fee_usd: usd,
        amount,
    });
}

/// Block reward first, then the community rewards this block's fees earned.
pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = mint_block(state, ctx);
    events.append(&mut state.rewards.pending);
    events
}
