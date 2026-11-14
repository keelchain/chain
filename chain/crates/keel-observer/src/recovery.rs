//! The recovery pass (docs/deposits.md, "Sent on the wrong network"):
//! every open row of `GET /v1/recovery` names a deposit index, a network,
//! a token and a destination its owner chose. The leading observer builds
//! the transfer out of the index's own address, signs it under the policy
//! (`SignContext::*Recover`, bound to the row), broadcasts it and
//! remembers it; the others find the same transfer on the external chain
//! by themselves. Once deep enough every observer reports it with
//! `ObserveRecovery`, and the quorum closes the row. No one decides
//! anything here: the chain already checked who owns the index.
//!
//! Gas comes from the index's address itself (the owner sends a little
//! native coin first) or, on a chain the observers serve, from the vault's
//! hot address through the same gas-funding contexts consolidation uses.
//! An EVM network the chain does not name (`evm_chain_id`) is served from
//! `[recovery] evm` of the config, with the address paying its own gas.

use crate::{
    chains::{
        eth::{self, EthRpc},
        sol::{self, SolRpc},
        tron::{self, TronApi},
        AddressBook,
    },
    config::{EthereumConfig, SolanaConfig, TronConfig},
    moves::{custodian_hex, forget, move_id, record, remember},
    outbound::OutboundContext,
    rpc::RecoveryRow,
    state::now_unix,
};
use keel_actions::{Action, Chain, RecoveryObservation};
use keel_chains::{eth::Eip1559Tx, policy::SignContext, tron::TronTxBuilder};

/// Gas for one ERC-20 transfer, the most a recovery transaction needs.
const EVM_GAS: u64 = 90_000;
/// A broadcast unknown to the node after this is rebuilt.
const STALE_SECS: u64 = 600;

fn key_of(row: &RecoveryRow) -> String {
    format!("recover:{}", row.id)
}

#[allow(clippy::too_many_arguments)]
async fn report(
    ctx: &OutboundContext<'_>,
    row: &RecoveryRow,
    tx_hash: [u8; 32],
    external_tx: Option<String>,
    amount: u128,
    external_height: u64,
    tip_height: u64,
    success: bool,
) -> anyhow::Result<()> {
    let key = format!("recovery:{}", row.id);
    let res = ctx
        .submitter
        .submit(
            Some(&key),
            Action::ObserveRecovery(RecoveryObservation {
                id: row.id,
                tx_hash,
                external_tx,
                amount,
                external_height,
                tip_height,
                success,
            }),
        )
        .await?;
    if res.admitted {
        tracing::info!(
            id = row.id,
            amount,
            success,
            "recovery observation submitted"
        );
    }
    Ok(())
}

fn hash32(txid: &str) -> anyhow::Result<[u8; 32]> {
    hex::decode(txid.trim_start_matches("0x"))?
        .try_into()
        .map_err(|_| anyhow::anyhow!("tx hash length"))
}

// ---------------------------------------------------------------- evm

/// Where an EVM recovery runs: a node of the right chain id and, when the
/// observers serve that chain, its config (hot index, gas funding).
pub struct EvmTarget<'a> {
    pub rpc: &'a dyn EthRpc,
    pub cfg: Option<&'a EthereumConfig>,
}

pub async fn evm(
    ctx: &OutboundContext<'_>,
    row: &RecoveryRow,
    book: &AddressBook,
    t: EvmTarget<'_>,
) -> anyhow::Result<()> {
    let rpc = t.rpc;
    let key = key_of(row);
    let index = row.deposit_index;
    let addr = book
        .address(index)
        .ok_or_else(|| anyhow::anyhow!("index {index} not derived"))?
        .to_string();
    let tip = eth::block_number(rpc).await?;
    let required = ctx.params.confirmations(book.chain) as u64;
    let token = row.token.as_deref();

    if let Some(tx) = record(ctx, &key) {
        match eth::receipt_status(rpc, &tx.txid).await? {
            Some(r) if tip.saturating_sub(r.block_number) >= required => {
                report(
                    ctx,
                    row,
                    hash32(&tx.txid)?,
                    None,
                    tx.amount.unwrap_or(0),
                    r.block_number,
                    tip,
                    r.success,
                )
                .await?;
                forget(ctx, &key)?;
            }
            Some(_) => {}
            None if tx.ours && now_unix().saturating_sub(tx.broadcast_unix_secs) > STALE_SECS => {
                tracing::warn!(id = row.id, txid = %tx.txid, "recovery transaction expired unconfirmed; rebuilding");
                forget(ctx, &key)?;
            }
            None => {}
        }
        return Ok(());
    }
    let gas_key = format!("recover-gas:{}", row.id);
    if let Some(tx) = record(ctx, &gas_key) {
        match eth::receipt_status(rpc, &tx.txid).await? {
            Some(_) => forget(ctx, &gas_key)?,
            None if now_unix().saturating_sub(tx.broadcast_unix_secs) > STALE_SECS => {
                forget(ctx, &gas_key)?
            }
            None => return Ok(()),
        }
    }
    if !ctx.may_build(&book.vault, move_id(&key)) {
        // Another observer may have moved it: any transfer from the index
        // to the destination, in the row's token, within the window.
        let window = t.cfg.map(|c| c.log_window).unwrap_or(2000);
        if let Some((txid, amount)) =
            eth::find_any_payment(rpc, token, &addr, &row.to, tip.saturating_sub(window), tip)
                .await?
        {
            remember(ctx, &key, book.chain, &txid, 0, amount, None)?;
        }
        return Ok(());
    }

    let chain_id = eth::chain_id(rpc).await?;
    if let Some(want) = row.evm_chain_id {
        if want != chain_id {
            anyhow::bail!("node serves chain id {chain_id}, the row wants {want}");
        }
    }
    let priority_fallback = t
        .cfg
        .map(|c| c.fallback_priority_wei)
        .unwrap_or(1_000_000_000);
    let (max_fee, priority) = eth::fee_params(rpc, priority_fallback).await?;
    let native = eth::balance(rpc, &addr).await?;
    let to20 = keel_chains::address::eth_decode(&row.to)?;
    let from20 = keel_chains::address::eth_decode(&addr)?;
    let (tx, amount) = match token {
        None => {
            let gas = max_fee.saturating_mul(21_000);
            if native <= gas {
                tracing::info!(id = row.id, addr, "recovery waits for gas at the address");
                return Ok(());
            }
            let value = native - gas;
            let nonce = eth::nonce(rpc, &addr).await?;
            (
                Eip1559Tx::native_transfer(chain_id, nonce, to20, value, max_fee, priority),
                value,
            )
        }
        Some(contract) => {
            let bal = eth::token_balance(rpc, contract, &addr).await?;
            if bal == 0 {
                tracing::info!(
                    id = row.id,
                    addr,
                    contract,
                    "nothing to recover at the address"
                );
                return Ok(());
            }
            let gas = max_fee.saturating_mul(EVM_GAS as u128);
            if native < gas {
                // On a served chain the hot address funds the gas, as it
                // does for consolidation; elsewhere the owner sends some.
                let Some(cfg) = t.cfg else {
                    tracing::info!(
                        id = row.id,
                        addr,
                        "token recovery waits for gas at the address"
                    );
                    return Ok(());
                };
                let hot = book
                    .address(cfg.hot_index)
                    .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
                    .to_string();
                let value = gas.min(keel_chains::policy::ETH_GAS_FUND_CAP_WEI);
                let nonce = eth::nonce(rpc, &hot).await?;
                let tx =
                    Eip1559Tx::native_transfer(chain_id, nonce, from20, value, max_fee, priority);
                let context = SignContext::EthGasFund {
                    custodian: custodian_hex(book),
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
                    crate::moves::eth_sign_and_send(ctx, rpc, book, cfg.hot_index, &tx, context)
                        .await?;
                remember(
                    ctx,
                    &gas_key,
                    book.chain,
                    &txid,
                    tx.max_gas_cost(),
                    value,
                    Some(hex::encode(raw)),
                )?;
                tracing::info!(id = row.id, %txid, "recovery gas funded from the hot address");
                return Ok(());
            }
            let nonce = eth::nonce(rpc, &addr).await?;
            let c20 = keel_chains::address::eth_decode(contract)?;
            (
                Eip1559Tx::erc20_transfer(chain_id, nonce, c20, to20, bal, max_fee, priority),
                bal,
            )
        }
    };
    let context = SignContext::EthRecover {
        recovery_id: row.id,
        index,
        chain_id,
        nonce: tx.nonce,
        max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
        max_fee_per_gas: tx.max_fee_per_gas,
        gas_limit: tx.gas_limit,
        to: format!("0x{}", hex::encode(tx.to)),
        value: tx.value,
        data: hex::encode(&tx.data),
    };
    let (txid, raw) = crate::moves::eth_sign_and_send(ctx, rpc, book, index, &tx, context).await?;
    remember(
        ctx,
        &key,
        book.chain,
        &txid,
        tx.max_gas_cost(),
        amount,
        Some(hex::encode(raw)),
    )?;
    tracing::info!(id = row.id, amount, %txid, "recovery broadcast");
    Ok(())
}

// ---------------------------------------------------------------- tron

pub async fn tron(
    ctx: &OutboundContext<'_>,
    row: &RecoveryRow,
    book: &AddressBook,
    api: &dyn TronApi,
    cfg: &TronConfig,
) -> anyhow::Result<()> {
    let key = key_of(row);
    let index = row.deposit_index;
    let addr = book
        .address(index)
        .ok_or_else(|| anyhow::anyhow!("index {index} not derived"))?
        .to_string();
    let now = tron::now_block(api).await?;
    let required = ctx.params.confirmations(Chain::Tron) as u64;

    if let Some(tx) = record(ctx, &key) {
        match tron::tx_info(api, &tx.txid).await? {
            Some(info) if now.number.saturating_sub(info.block_number) >= required => {
                report(
                    ctx,
                    row,
                    hash32(&tx.txid)?,
                    None,
                    tx.amount.unwrap_or(0),
                    info.block_number,
                    now.number,
                    info.success,
                )
                .await?;
                forget(ctx, &key)?;
            }
            Some(_) => {}
            None if tx.ours && now_unix().saturating_sub(tx.broadcast_unix_secs) > STALE_SECS => {
                forget(ctx, &key)?;
            }
            None => {}
        }
        return Ok(());
    }
    let gas_key = format!("recover-gas:{}", row.id);
    if let Some(tx) = record(ctx, &gas_key) {
        match tron::tx_info(api, &tx.txid).await? {
            Some(_) => forget(ctx, &gas_key)?,
            None if now_unix().saturating_sub(tx.broadcast_unix_secs) > STALE_SECS => {
                forget(ctx, &gas_key)?
            }
            None => return Ok(()),
        }
    }
    if !ctx.may_build(&book.vault, move_id(&key)) {
        let since = now.timestamp_ms.saturating_sub(24 * 3600 * 1000);
        if let Some((txid, amount)) =
            tron::find_transfer_from(api, row.token.as_deref(), &addr, &row.to, since).await?
        {
            remember(ctx, &key, Chain::Tron, &txid, 0, amount, None)?;
        }
        return Ok(());
    }

    let (trx, trc20) = tron::account_balances(api, &addr).await?;
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
    let owner = t21(&addr)?;
    let dest = t21(&row.to)?;
    let (raw, amount) = match row.token.as_deref() {
        None => {
            let keep = cfg.fee_sun as u128;
            if trx <= keep {
                tracing::info!(id = row.id, addr, "recovery waits for TRX at the address");
                return Ok(());
            }
            let amount = trx - keep;
            (builder.transfer(&owner, &dest, amount as u64), amount)
        }
        Some(contract) => {
            let bal = trc20
                .iter()
                .find(|(c, _)| {
                    c.eq_ignore_ascii_case(contract)
                        || tron::to_base58(c).ok().as_deref() == Some(contract)
                })
                .map(|(_, n)| *n)
                .unwrap_or(0);
            if bal == 0 {
                tracing::info!(
                    id = row.id,
                    addr,
                    contract,
                    "nothing to recover at the address"
                );
                return Ok(());
            }
            if trx < cfg.fee_limit_sun as u128 {
                let hot = book
                    .address(cfg.hot_index)
                    .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
                    .to_string();
                let gas = keel_chains::policy::TRON_GAS_FUND_CAP_SUN.min(cfg.fee_limit_sun as u128)
                    as u64;
                let fund = builder.transfer(&t21(&hot)?, &owner, gas);
                let context = SignContext::TronGasFund {
                    custodian: custodian_hex(book),
                    index,
                    raw_data: hex::encode(fund.to_bytes()),
                };
                let (txid, json) =
                    crate::moves::tron_sign_and_send(ctx, api, book, cfg.hot_index, &fund, context)
                        .await?;
                remember(
                    ctx,
                    &gas_key,
                    Chain::Tron,
                    &txid,
                    cfg.fee_sun as u128,
                    gas as u128,
                    Some(json),
                )?;
                tracing::info!(id = row.id, %txid, "recovery TRX funded from the hot address");
                return Ok(());
            }
            (
                builder.trc20_transfer(&owner, &t21(contract)?, &dest, bal),
                bal,
            )
        }
    };
    let context = SignContext::TronRecover {
        recovery_id: row.id,
        index,
        raw_data: hex::encode(raw.to_bytes()),
    };
    let (txid, json) =
        crate::moves::tron_sign_and_send(ctx, api, book, index, &raw, context).await?;
    remember(
        ctx,
        &key,
        Chain::Tron,
        &txid,
        cfg.fee_sun as u128,
        amount,
        Some(json),
    )?;
    tracing::info!(id = row.id, amount, %txid, "recovery broadcast");
    Ok(())
}

// ---------------------------------------------------------------- solana

pub async fn solana(
    ctx: &OutboundContext<'_>,
    row: &RecoveryRow,
    book: &AddressBook,
    rpc: &dyn SolRpc,
    cfg: &SolanaConfig,
) -> anyhow::Result<()> {
    let key = key_of(row);
    let index = row.deposit_index;
    let addr = book
        .address(index)
        .ok_or_else(|| anyhow::anyhow!("index {index} not derived"))?
        .to_string();
    let tip = sol::slot(rpc).await?;
    let required = ctx.params.confirmations(Chain::Solana) as u64;

    if let Some(tx) = record(ctx, &key) {
        match sol::signature_status(rpc, &tx.txid).await? {
            Some((slot, true, failed)) if tip.saturating_sub(slot) >= required => {
                let sig = sol::decode_signature(&tx.txid)?;
                report(
                    ctx,
                    row,
                    sol::tx_hash_of(&sig),
                    Some(tx.txid.clone()),
                    tx.amount.unwrap_or(0),
                    slot,
                    tip,
                    !failed,
                )
                .await?;
                forget(ctx, &key)?;
            }
            Some(_) => {}
            None if now_unix().saturating_sub(tx.broadcast_unix_secs) > 90 => forget(ctx, &key)?,
            None => {}
        }
        return Ok(());
    }
    if !ctx.may_build(&book.vault, move_id(&key)) {
        for s in sol::signatures(rpc, &addr, cfg.signature_limit).await? {
            if s.failed {
                continue;
            }
            let Some(view) = sol::transaction(rpc, &s.signature).await? else {
                continue;
            };
            let moved = match &row.token {
                None => view
                    .lamport_delta(&row.to)
                    .filter(|(_, d)| *d > 0)
                    .map(|(_, d)| d as u128),
                Some(mint) => view
                    .token_deltas(&row.to)
                    .into_iter()
                    .find(|(_, m, d)| m == mint && *d > 0)
                    .map(|(_, _, d)| d as u128),
            };
            if let Some(amount) = moved {
                remember(ctx, &key, Chain::Solana, &s.signature, 0, amount, None)?;
                break;
            }
        }
        return Ok(());
    }

    let addr32 = keel_chains::sol::decode(&addr)?;
    let dest = keel_chains::sol::decode(&row.to)?;
    let lamports = sol::balance(rpc, &addr).await?;
    let fee = sol::fee_estimate(cfg, 2);
    let hot = book
        .address(cfg.hot_index)
        .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
        .to_string();
    let hot32 = keel_chains::sol::decode(&hot)?;
    let mut ixs = Vec::new();
    if cfg.priority_micro_lamports > 0 {
        ixs.push(keel_chains::sol::set_compute_unit_price(
            cfg.priority_micro_lamports,
        ));
    }
    let (payer, amount) = match &row.token {
        None => {
            if lamports <= fee {
                tracing::info!(id = row.id, addr, "nothing to recover at the address");
                return Ok(());
            }
            let value = lamports - fee;
            ixs.push(keel_chains::sol::transfer_lamports(&addr32, &dest, value));
            (addr32, value as u128)
        }
        Some(mint) => {
            let mint32 = keel_chains::sol::decode(mint)?;
            let bal = sol::token_account_balance(rpc, &sol::ata(&addr, mint)?).await?;
            if bal == 0 {
                tracing::info!(id = row.id, addr, mint, "nothing to recover at the address");
                return Ok(());
            }
            let decimals = match cfg.tokens.iter().find(|t| &t.contract == mint) {
                Some(t) => t.decimals.unwrap_or(6),
                None => sol::mint_decimals(rpc, mint).await?,
            };
            // The index pays when it can (fee plus the destination's
            // account rent); else the hot address does.
            let payer = if lamports >= fee + keel_chains::sol::ATA_RENT_LAMPORTS {
                addr32
            } else {
                hot32
            };
            ixs.push(keel_chains::sol::create_associated_token_account(
                &payer, &dest, &mint32,
            )?);
            ixs.push(keel_chains::sol::transfer_checked(
                &keel_chains::sol::associated_token_account(&addr32, &mint32)?,
                &mint32,
                &keel_chains::sol::associated_token_account(&dest, &mint32)?,
                &addr32,
                bal,
                decimals,
            ));
            (payer, bal as u128)
        }
    };
    let (blockhash, _) = sol::latest_blockhash(rpc).await?;
    let msg = keel_chains::sol::build_message(&payer, &ixs, blockhash)?;
    let context = SignContext::SolRecover {
        recovery_id: row.id,
        index,
        message: hex::encode(msg.serialize()),
    };
    let (txid, raw) =
        crate::moves::sol_sign_and_send(ctx, rpc, book, &[index, cfg.hot_index], &msg, context)
            .await?;
    remember(
        ctx,
        &key,
        Chain::Solana,
        &txid,
        fee as u128,
        amount,
        Some(raw),
    )?;
    tracing::info!(id = row.id, amount, %txid, "recovery broadcast");
    Ok(())
}
