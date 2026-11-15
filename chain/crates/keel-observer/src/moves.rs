//! Custody moves that are not payouts: consolidating EVM (Ethereum, BSC),
//! Tron and Solana deposit indexes into the *same key's* hot address
//! (funding their gas first where the chain needs it), and reporting a
//! draining generation empty so the chain can retire it.
//!
//! Every move stays inside one generation of one vault. Generations are
//! separate pools: nothing here ever sends a coin from one generation to
//! another, and the signing policy (`keel_chains::policy`) cannot even
//! describe such a transfer: a consolidation may only pay the signing
//! key's own hot address, gas only the key's own index within a cap. A
//! draining generation empties through the payouts the chain points at it
//! and through nothing else.
//!
//! Every move is built by one observer at a time (the same leader rule as
//! batches, keyed by the move's label), signed by the generation's own
//! signer, and remembered in the state file until it confirms.

use crate::{
    chains::{
        btc::{self, BitcoinRpc},
        eth::{self, EthRpc},
        sol::{self, SolRpc},
        tron::{self, TronApi},
        AddressBook,
    },
    config::{EthereumConfig, SolanaConfig, TronConfig},
    outbound::OutboundContext,
    state::{now_unix, OutboundTx},
};
use keel_actions::{Action, Chain};
use keel_chains::{eth::Eip1559Tx, policy::SignContext, tron::TronTxBuilder};
use std::collections::BTreeSet;

/// Gas an EVM deposit index gets when it needs some: enough for one
/// ERC-20 transfer at the current fee, which is what the index does next.
const ETH_CONSOLIDATE_GAS: u64 = 65_000;
/// TRX sent to a Tron deposit index so it can move its tokens (energy and
/// bandwidth are burnt from it); within the policy's cap.
const TRON_GAS_FUND_SUN: u64 = 30_000_000;

pub(crate) fn move_id(key: &str) -> u64 {
    let h = keel_crypto::sha256(&[b"move", key.as_bytes()]);
    u64::from_le_bytes(h[..8].try_into().expect("8 bytes"))
}

pub(crate) fn record(ctx: &OutboundContext<'_>, key: &str) -> Option<OutboundTx> {
    ctx.state.read(|d| d.moves.get(key).cloned())
}

pub(crate) fn remember(
    ctx: &OutboundContext<'_>,
    key: &str,
    chain: Chain,
    txid: &str,
    fee_hint: u128,
    amount: u128,
    raw: Option<String>,
) -> anyhow::Result<()> {
    let tx = OutboundTx {
        chain: chain.as_str().into(),
        txid: txid.to_string(),
        broadcast_unix_secs: now_unix(),
        fee_hint,
        ours: true,
        raw,
        amount: Some(amount),
    };
    ctx.state.update(|d| {
        d.moves.insert(key.to_string(), tx);
    })
}

pub(crate) fn forget(ctx: &OutboundContext<'_>, key: &str) -> anyhow::Result<()> {
    ctx.state.update(|d| {
        d.moves.remove(key);
    })
}

fn in_flight(ctx: &OutboundContext<'_>, prefix: &str) -> bool {
    ctx.state
        .read(|d| d.moves.keys().any(|k| k.starts_with(prefix)))
}

/// Tell the chain a draining generation holds nothing any more, once.
async fn report_empty(ctx: &OutboundContext<'_>, from: &AddressBook) -> anyhow::Result<()> {
    let Some(custodian) = from.vault.custodian else {
        return Ok(());
    };
    let key = format!("empty:{}", from.key_prefix());
    if ctx.state.is_submitted(&key) {
        return Ok(());
    }
    let res = ctx
        .submitter
        .submit(
            Some(&key),
            Action::RetireCustodyGeneration {
                chain: from.chain,
                custodian,
                gen: from.vault.gen,
            },
        )
        .await?;
    if res.admitted {
        tracing::info!(key, tx_id = ?res.tx_id, "draining generation reported empty");
    }
    Ok(())
}

pub(crate) fn custodian_hex(book: &AddressBook) -> String {
    book.vault.custodian.map(|c| c.to_hex()).unwrap_or_default()
}

// ---------------------------------------------------------------- bitcoin

/// One Bitcoin pass for a custody book: report a draining generation empty
/// once nothing is left. Coins leave it through payouts alone (a batch
/// spends only that generation's UTXOs and returns change to its own
/// key); this pass never moves anything.
pub async fn bitcoin(
    ctx: &OutboundContext<'_>,
    rpc: &dyn BitcoinRpc,
    from: &AddressBook,
) -> anyhow::Result<()> {
    if !from.vault.is_draining() {
        return Ok(());
    }
    let utxos = btc::vault_utxos(&btc::list_unspent(rpc, 1).await?, from)?;
    if utxos.is_empty() && !in_flight(ctx, &from.key_prefix()) {
        // Every observer reports it; the quorum retires it after the grace.
        report_empty(ctx, from).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------- ethereum

fn eth20(addr: &str) -> anyhow::Result<[u8; 20]> {
    Ok(keel_chains::address::eth_decode(addr)?)
}

pub(crate) async fn eth_sign_and_send(
    ctx: &OutboundContext<'_>,
    rpc: &dyn EthRpc,
    book: &AddressBook,
    index: u64,
    tx: &Eip1559Tx,
    context: SignContext,
) -> anyhow::Result<(String, Vec<u8>)> {
    let digest = tx.signing_hash();
    let sig = ctx
        .tss
        .sign(digest, &book.path(index), Some(&context))
        .await?;
    let v = keel_chains::eth::recovery_id(&digest, &sig.compact(), &book.pubkey(index)?)?;
    let raw = tx.raw_signed(&sig.compact(), v);
    let hash = keel_chains::eth::tx_hash(&raw);
    let txid = format!("0x{}", hex::encode(hash));
    eth::send_raw(rpc, &raw).await?;
    Ok((txid, raw))
}

/// Settle a remembered Ethereum move: `Some((fee, success))` once it is
/// deep enough, `None` while pending.
async fn eth_settled(
    ctx: &OutboundContext<'_>,
    chain: Chain,
    rpc: &dyn EthRpc,
    key: &str,
    tip: u64,
) -> anyhow::Result<Option<(OutboundTx, u128, bool)>> {
    let required = ctx.params.confirmations(chain) as u64;
    let Some(tx) = record(ctx, key) else {
        return Ok(None);
    };
    if let Some(r) = eth::receipt_status(rpc, &tx.txid).await? {
        if tip.saturating_sub(r.block_number) >= required {
            return Ok(Some((tx, r.fee_paid, r.success)));
        }
    }
    Ok(None)
}

/// One pass over an EVM chain (the book's: Ethereum or BSC) for a custody
/// book: consolidate every deposit index with a balance into this key's
/// own hot address, and report a draining generation empty.
pub async fn evm(
    ctx: &OutboundContext<'_>,
    rpc: &dyn EthRpc,
    from: &AddressBook,
    cfg: &EthereumConfig,
) -> anyhow::Result<()> {
    let chain = from.chain;
    let native = crate::chains::native_asset(chain);
    let tag = format!(":{}", chain.as_str());
    let tip = eth::block_number(rpc).await?;
    let hot = from
        .address(cfg.hot_index)
        .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
        .to_string();
    let prefix = from.key_prefix();
    let custodian = custodian_hex(from);
    let mut found_funds = false;
    let chain_id = eth::chain_id(rpc).await?;
    let (max_fee, priority) = eth::fee_params(rpc, cfg.fallback_priority_wei).await?;

    // Settle what is in flight first.
    let keys: Vec<String> = ctx.state.read(|d| {
        d.moves
            .keys()
            .filter(|k| k.starts_with(&prefix) && k[prefix.len()..].contains(tag.as_str()))
            .cloned()
            .collect()
    });
    for key in keys {
        if eth_settled(ctx, chain, rpc, &key, tip).await?.is_some() {
            forget(ctx, &key)?;
        } else {
            found_funds = true; // something still moving
        }
    }

    let assets: Vec<(String, Option<String>)> = std::iter::once((native.to_string(), None))
        .chain(cfg.tokens.iter().map(|t| {
            (
                format!("{}.{}", chain.as_str(), t.symbol),
                Some(t.contract.clone()),
            )
        }))
        .collect();
    let gas_cost = (ETH_CONSOLIDATE_GAS as u128).saturating_mul(max_fee);

    // Deposit indexes: fund gas, then move each asset to our own hot address.
    for (index, addr) in from.addresses() {
        if index == cfg.hot_index || from.owner(index).is_none() {
            continue;
        }
        let addr = addr.to_string();
        let native = eth::balance(rpc, &addr).await?;
        for (asset, contract) in &assets {
            let bal = match contract {
                Some(c) => eth::token_balance(rpc, c, &addr).await?,
                None => native.saturating_sub(gas_cost),
            };
            if bal == 0 {
                continue;
            }
            found_funds = true;
            let key = format!("{prefix}cons:{}:{asset}:{index}", chain.as_str());
            let gas_key = format!("{prefix}gas:{}:{index}", chain.as_str());
            if record(ctx, &key).is_some() || record(ctx, &gas_key).is_some() {
                continue;
            }
            if !ctx.may_build(&from.vault, move_id(&key)) {
                continue;
            }
            if contract.is_some() && native < gas_cost {
                // Gas first, from the hot address.
                let value = gas_cost.min(keel_chains::policy::ETH_GAS_FUND_CAP_WEI);
                let nonce = eth::nonce(rpc, &hot).await?;
                let tx = Eip1559Tx::native_transfer(
                    chain_id,
                    nonce,
                    eth20(&addr)?,
                    value,
                    max_fee,
                    priority,
                );
                let context = SignContext::EthGasFund {
                    custodian: custodian.clone(),
                    index,
                    chain_id,
                    nonce,
                    max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
                    max_fee_per_gas: tx.max_fee_per_gas,
                    gas_limit: tx.gas_limit,
                    to: addr.clone(),
                    value,
                };
                let (txid, raw) =
                    eth_sign_and_send(ctx, rpc, from, cfg.hot_index, &tx, context).await?;
                remember(
                    ctx,
                    &gas_key,
                    chain,
                    &txid,
                    tx.max_gas_cost(),
                    value,
                    Some(hex::encode(raw)),
                )?;
                tracing::info!(chain = chain.as_str(), index, %txid, "evm gas funded for consolidation");
                break;
            }
            let nonce = eth::nonce(rpc, &addr).await?;
            let to20 = eth20(&hot)?;
            let tx = match contract {
                Some(c) => Eip1559Tx::erc20_transfer(
                    chain_id,
                    nonce,
                    eth20(c)?,
                    to20,
                    bal,
                    max_fee,
                    priority,
                ),
                None => Eip1559Tx::native_transfer(chain_id, nonce, to20, bal, max_fee, priority),
            };
            let context = SignContext::EthConsolidate {
                custodian: custodian.clone(),
                index,
                chain_id,
                nonce,
                max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
                max_fee_per_gas: tx.max_fee_per_gas,
                gas_limit: tx.gas_limit,
                to: format!("0x{}", hex::encode(tx.to)),
                value: tx.value,
                data: hex::encode(&tx.data),
            };
            let (txid, raw) = eth_sign_and_send(ctx, rpc, from, index, &tx, context).await?;
            remember(
                ctx,
                &key,
                chain,
                &txid,
                tx.max_gas_cost(),
                bal,
                Some(hex::encode(raw)),
            )?;
            tracing::info!(chain = chain.as_str(), index, asset, amount = bal, %txid, "evm consolidation broadcast");
        }
    }

    if from.vault.is_draining() {
        let native = eth::balance(rpc, &hot).await?;
        if native > gas_cost {
            found_funds = true;
        }
        for t in &cfg.tokens {
            if eth::token_balance(rpc, &t.contract, &hot).await? > 0 {
                found_funds = true;
            }
        }
    }
    if from.vault.is_draining() && !found_funds && !in_flight(ctx, &prefix) {
        report_empty(ctx, from).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------- tron

pub(crate) async fn tron_sign_and_send(
    ctx: &OutboundContext<'_>,
    api: &dyn TronApi,
    book: &AddressBook,
    index: u64,
    raw: &keel_chains::tron::Raw,
    context: SignContext,
) -> anyhow::Result<(String, String)> {
    let digest = raw.txid();
    let sig = ctx
        .tss
        .sign(digest, &book.path(index), Some(&context))
        .await?;
    let v = keel_chains::eth::recovery_id(&digest, &sig.compact(), &book.pubkey(index)?)?;
    let sig65 = keel_chains::tron::Raw::signature(&sig.compact(), v);
    let signed = raw.signed_hex(sig65);
    tron::broadcast(api, &signed).await?;
    Ok((hex::encode(digest), signed))
}

/// One Tron pass for a custody book (see [`evm`]).
pub async fn tron(
    ctx: &OutboundContext<'_>,
    api: &dyn TronApi,
    from: &AddressBook,
    cfg: &TronConfig,
) -> anyhow::Result<()> {
    let now = tron::now_block(api).await?;
    let required = ctx.params.confirmations(Chain::Tron) as u64;
    let hot = from
        .address(cfg.hot_index)
        .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
        .to_string();
    let prefix = from.key_prefix();
    let custodian = custodian_hex(from);
    let mut found_funds = false;
    let (ref_block_bytes, ref_block_hash) = keel_chains::tron::ref_block(now.number, &now.id);
    let timestamp_ms = now.timestamp_ms.max(now_unix() * 1000);
    let builder = TronTxBuilder {
        ref_block_bytes,
        ref_block_hash,
        timestamp_ms,
        expiration_ms: timestamp_ms + 60_000,
        fee_limit_sun: cfg.fee_limit_sun,
    };
    let t21 = |a: &str| keel_chains::address::tron_decode(a);

    let keys: Vec<String> = ctx.state.read(|d| {
        d.moves
            .keys()
            .filter(|k| k.starts_with(&prefix) && k[prefix.len()..].contains(":TRON"))
            .cloned()
            .collect()
    });
    for key in keys {
        let Some(tx) = record(ctx, &key) else {
            continue;
        };
        match tron::tx_info(api, &tx.txid).await? {
            Some(info) if now.number.saturating_sub(info.block_number) >= required => {
                forget(ctx, &key)?;
            }
            _ => found_funds = true,
        }
    }

    let tokens: Vec<(String, String)> = cfg
        .tokens
        .iter()
        .map(|t| (format!("TRON.{}", t.symbol), t.contract.clone()))
        .collect();

    for (index, addr) in from.addresses() {
        if index == cfg.hot_index || from.owner(index).is_none() {
            continue;
        }
        let addr = addr.to_string();
        let (trx, trc20) = tron::account_balances(api, &addr).await?;
        // Tokens first (they need TRX for energy), then the TRX itself.
        let mut moves: Vec<(String, Option<String>, u128)> = tokens
            .iter()
            .filter_map(|(asset, contract)| {
                let bal = trc20
                    .iter()
                    .find(|(c, _)| {
                        c.eq_ignore_ascii_case(contract)
                            || tron::to_base58(c).ok().as_deref() == Some(contract.as_str())
                    })
                    .map(|(_, n)| *n)
                    .unwrap_or(0);
                (bal > 0).then(|| (asset.clone(), Some(contract.clone()), bal))
            })
            .collect();
        if moves.is_empty() && trx > TRON_GAS_FUND_SUN as u128 {
            moves.push(("TRON.TRX".into(), None, trx - cfg.fee_sun as u128));
        }
        for (asset, contract, bal) in moves {
            found_funds = true;
            let key = format!("{prefix}cons:TRON:{asset}:{index}");
            let gas_key = format!("{prefix}gas:TRON:{index}");
            if record(ctx, &key).is_some() || record(ctx, &gas_key).is_some() {
                continue;
            }
            if !ctx.may_build(&from.vault, move_id(&key)) {
                continue;
            }
            if contract.is_some() && trx < cfg.fee_limit_sun as u128 {
                let raw = builder.transfer(&t21(&hot)?, &t21(&addr)?, TRON_GAS_FUND_SUN);
                let context = SignContext::TronGasFund {
                    custodian: custodian.clone(),
                    index,
                    raw_data: hex::encode(raw.to_bytes()),
                };
                let (txid, json) =
                    tron_sign_and_send(ctx, api, from, cfg.hot_index, &raw, context).await?;
                remember(
                    ctx,
                    &gas_key,
                    Chain::Tron,
                    &txid,
                    cfg.fee_sun as u128,
                    TRON_GAS_FUND_SUN as u128,
                    Some(json.to_string()),
                )?;
                tracing::info!(index, %txid, "tron gas funded for consolidation");
                break;
            }
            let owner = t21(&addr)?;
            let dest = t21(&hot)?;
            let raw = match &contract {
                Some(c) => builder.trc20_transfer(&owner, &t21(c)?, &dest, bal),
                None => builder.transfer(&owner, &dest, bal as u64),
            };
            let context = SignContext::TronConsolidate {
                custodian: custodian.clone(),
                index,
                raw_data: hex::encode(raw.to_bytes()),
            };
            let (txid, json) = tron_sign_and_send(ctx, api, from, index, &raw, context).await?;
            remember(
                ctx,
                &key,
                Chain::Tron,
                &txid,
                cfg.fee_sun as u128,
                bal,
                Some(json.to_string()),
            )?;
            tracing::info!(index, asset, amount = bal, %txid, "tron consolidation broadcast");
        }
    }

    let (trx, trc20) = tron::account_balances(api, &hot).await?;
    if from.vault.is_draining()
        && (trx > cfg.fee_limit_sun as u128 || trc20.values().any(|n| *n > 0))
    {
        found_funds = true;
    }
    if from.vault.is_draining() && !found_funds && !in_flight(ctx, &prefix) {
        report_empty(ctx, from).await?;
    }
    let _ = BTreeSet::<u64>::new();
    Ok(())
}

// ---------------------------------------------------------------- solana

/// `(asset, Some((mint, decimals)) for a token, amount)` of one move.
type SolMove = (String, Option<(String, u8)>, u64);

/// Sign a Solana message with every signer it names (each a child of
/// this book: the hot address pays, a moved index authorizes), broadcast
/// it, and return `(signature base58, raw base64)`.
pub(crate) async fn sol_sign_and_send(
    ctx: &OutboundContext<'_>,
    rpc: &dyn SolRpc,
    book: &AddressBook,
    indexes: &[u64],
    message: &keel_chains::sol::Message,
    context: SignContext,
) -> anyhow::Result<(String, String)> {
    let bytes = message.serialize();
    let mut sigs = Vec::with_capacity(message.signers().len());
    for signer in message.signers() {
        let index = indexes
            .iter()
            .copied()
            .find(|i| book.ed25519_pubkey(*i).ok().as_ref() == Some(signer))
            .ok_or_else(|| anyhow::anyhow!("message signer is not a key of this book"))?;
        sigs.push(
            ctx.tss
                .sign_message(&bytes, &book.path(index), Some(&context))
                .await?,
        );
    }
    let raw = keel_chains::sol::serialize_transaction(&bytes, &sigs);
    let txid = keel_chains::sol::transaction_id(&sigs[0]);
    sol::send_transaction(rpc, &raw).await?;
    use base64::Engine as _;
    Ok((txid, base64::engine::general_purpose::STANDARD.encode(raw)))
}

/// One Solana pass for a custody book (see [`evm`]): every deposit index
/// with a balance is moved to this key's own hot address, paid by that hot
/// address so no gas funding is needed; a draining generation holding
/// nothing is reported empty.
pub async fn solana(
    ctx: &OutboundContext<'_>,
    rpc: &dyn SolRpc,
    from: &AddressBook,
    cfg: &SolanaConfig,
) -> anyhow::Result<()> {
    let tip = sol::slot(rpc).await?;
    let required = ctx.params.confirmations(Chain::Solana) as u64;
    let hot = from
        .address(cfg.hot_index)
        .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
        .to_string();
    let hot32 = keel_chains::sol::decode(&hot)?;
    let prefix = from.key_prefix();
    let custodian = custodian_hex(from);
    let mut found_funds = false;
    let tokens: Vec<(String, String, u8)> = cfg
        .tokens
        .iter()
        .map(|t| {
            (
                format!("SOL.{}", t.symbol),
                t.contract.clone(),
                t.decimals.unwrap_or(6),
            )
        })
        .collect();

    // Settle what is in flight.
    let keys: Vec<String> = ctx.state.read(|d| {
        d.moves
            .keys()
            .filter(|k| k.starts_with(&prefix) && k[prefix.len()..].contains(":SOL"))
            .cloned()
            .collect()
    });
    for key in keys {
        let Some(tx) = record(ctx, &key) else {
            continue;
        };
        match sol::signature_status(rpc, &tx.txid).await? {
            Some((slot, true, _)) if tip.saturating_sub(slot) >= required => {
                forget(ctx, &key)?;
            }
            None if now_unix().saturating_sub(tx.broadcast_unix_secs) > 90 => {
                tracing::warn!(key, txid = %tx.txid, "solana move expired unconfirmed; rebuilding");
                forget(ctx, &key)?;
            }
            _ => found_funds = true,
        }
    }

    let fee_hint = sol::fee_estimate(cfg, 2) as u128;
    let (blockhash, _) = sol::latest_blockhash(rpc).await?;
    let priority = || {
        (cfg.priority_micro_lamports > 0)
            .then(|| keel_chains::sol::set_compute_unit_price(cfg.priority_micro_lamports))
    };

    // Deposit indexes → our own hot address, which pays.
    for (index, addr) in from.addresses() {
        if index == cfg.hot_index || from.owner(index).is_none() {
            continue;
        }
        let addr = addr.to_string();
        let addr32 = keel_chains::sol::decode(&addr)?;
        let mut moves: Vec<SolMove> = Vec::new();
        for (asset, mint, decimals) in &tokens {
            let bal = sol::token_account_balance(rpc, &sol::ata(&addr, mint)?).await?;
            if bal > 0 {
                moves.push((asset.clone(), Some((mint.clone(), *decimals)), bal));
            }
        }
        let lamports = sol::balance(rpc, &addr).await?;
        if lamports > 0 {
            moves.push(("SOL.SOL".into(), None, lamports));
        }
        for (asset, token, bal) in moves {
            found_funds = true;
            let key = format!("{prefix}cons:SOL:{asset}:{index}");
            if record(ctx, &key).is_some() || !ctx.may_build(&from.vault, move_id(&key)) {
                continue;
            }
            let mut ixs: Vec<keel_chains::sol::Instruction> = priority().into_iter().collect();
            match &token {
                Some((mint, decimals)) => {
                    let mint32 = keel_chains::sol::decode(mint)?;
                    ixs.push(keel_chains::sol::create_associated_token_account(
                        &hot32, &hot32, &mint32,
                    )?);
                    ixs.push(keel_chains::sol::transfer_checked(
                        &keel_chains::sol::associated_token_account(&addr32, &mint32)?,
                        &mint32,
                        &keel_chains::sol::associated_token_account(&hot32, &mint32)?,
                        &addr32,
                        bal,
                        *decimals,
                    ));
                }
                None => ixs.push(keel_chains::sol::transfer_lamports(&addr32, &hot32, bal)),
            }
            let msg = keel_chains::sol::build_message(&hot32, &ixs, blockhash)?;
            let context = SignContext::SolConsolidate {
                custodian: custodian.clone(),
                index,
                message: hex::encode(msg.serialize()),
            };
            let (txid, raw) =
                sol_sign_and_send(ctx, rpc, from, &[cfg.hot_index, index], &msg, context).await?;
            remember(
                ctx,
                &key,
                Chain::Solana,
                &txid,
                fee_hint,
                bal as u128,
                Some(raw),
            )?;
            tracing::info!(index, asset, amount = bal, %txid, "solana consolidation broadcast");
        }
    }

    let hot_lamports = sol::balance(rpc, &hot).await?;
    if from.vault.is_draining() {
        if hot_lamports > fee_hint as u64 {
            found_funds = true;
        }
        for (_, mint, _) in &tokens {
            if sol::token_account_balance(rpc, &sol::ata(&hot, mint)?).await? > 0 {
                found_funds = true;
            }
        }
    }
    if from.vault.is_draining() && !found_funds && !in_flight(ctx, &prefix) {
        report_empty(ctx, from).await?;
    }
    Ok(())
}
