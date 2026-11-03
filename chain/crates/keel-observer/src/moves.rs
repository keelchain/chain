//! Custody moves that are not payouts: sweeping a draining generation of a
//! client's vault to its active generation, consolidating Ethereum and
//! Tron deposit indexes into the hot address (funding their gas first),
//! and reporting a draining generation empty so the chain can retire it.
//!
//! Every move is built by one observer at a time (the same leader rule as
//! batches, keyed by the move's label), signed by the *from* generation's
//! own signer under `keel_chains::policy` (a sweep may only pay the active
//! generation's own address, a consolidation only a hot address, gas only
//! the key's own index within a cap), and remembered in the state file
//! until it confirms. Sweeps are then attested with `ObserveCustodySweep`.

use crate::{
    chains::{
        btc::{self, BitcoinRpc},
        eth::{self, EthRpc},
        tron::{self, TronApi},
        AddressBook,
    },
    config::{BitcoinConfig, EthereumConfig, TronConfig},
    outbound::OutboundContext,
    state::{now_unix, OutboundTx},
};
use keel_actions::{Action, Chain, SweepObservation};
use keel_chains::{eth::Eip1559Tx, policy::SignContext, tron::TronTxBuilder};
use std::collections::BTreeSet;

/// Gas an Ethereum deposit index gets when it needs some: enough for one
/// ERC-20 transfer at the current fee, which is what the index does next.
const ETH_CONSOLIDATE_GAS: u64 = 65_000;
/// TRX sent to a Tron deposit index so it can move its tokens (energy and
/// bandwidth are burnt from it); within the policy's cap.
const TRON_GAS_FUND_SUN: u64 = 30_000_000;

fn move_id(key: &str) -> u64 {
    let h = keel_crypto::sha256(&[b"move", key.as_bytes()]);
    u64::from_le_bytes(h[..8].try_into().expect("8 bytes"))
}

fn record(ctx: &OutboundContext<'_>, key: &str) -> Option<OutboundTx> {
    ctx.state.read(|d| d.moves.get(key).cloned())
}

fn remember(
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

fn forget(ctx: &OutboundContext<'_>, key: &str) -> anyhow::Result<()> {
    ctx.state.update(|d| {
        d.moves.remove(key);
    })
}

fn in_flight(ctx: &OutboundContext<'_>, prefix: &str) -> bool {
    ctx.state
        .read(|d| d.moves.keys().any(|k| k.starts_with(prefix)))
}

/// Attest a confirmed sweep once.
async fn report_sweep(
    ctx: &OutboundContext<'_>,
    from: &AddressBook,
    asset: &str,
    txid_hex: &str,
    tx_hash: [u8; 32],
    amount: u128,
    fee: u128,
) -> anyhow::Result<()> {
    let Some(custodian) = from.vault.custodian else {
        return Ok(());
    };
    let key = format!("sweep:{}{txid_hex}", from.key_prefix());
    if ctx.state.is_submitted(&key) {
        return Ok(());
    }
    let action = Action::ObserveCustodySweep(SweepObservation {
        chain: from.chain,
        custodian,
        from_gen: from.vault.gen,
        tx_hash,
        asset: keel_types::Asset::new(asset),
        amount,
        fee,
    });
    let res = ctx.submitter.submit(Some(&key), action).await?;
    if res.admitted {
        tracing::info!(key, tx_id = ?res.tx_id, "custody sweep attested");
    }
    Ok(())
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

/// Sweeps other observers reported for `from`: verify each on this node
/// and vote when it holds. `check` decides whether the transaction indeed
/// moved `amount` of `asset` from this generation to the active one and
/// is deep enough, and returns the fee it paid.
async fn verify_pending<F, Fut>(
    ctx: &OutboundContext<'_>,
    from: &AddressBook,
    check: F,
) -> anyhow::Result<()>
where
    F: Fn(crate::rpc::PendingSweep) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<u128>>>,
{
    for p in from.vault.pending_sweeps.clone() {
        if p.from_gen != from.vault.gen {
            continue;
        }
        let key = format!("sweep:{}{}", from.key_prefix(), p.tx_hash_hex);
        if ctx.state.is_submitted(&key) {
            continue;
        }
        let Ok(bytes) = hex::decode(p.tx_hash_hex.trim_start_matches("0x")) else {
            continue;
        };
        let Ok(hash) = <[u8; 32]>::try_from(bytes.as_slice()) else {
            continue;
        };
        match check(p.clone()).await {
            Ok(Some(fee)) => {
                report_sweep(ctx, from, &p.asset, &p.tx_hash_hex, hash, p.amount, fee).await?;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, tx = %p.tx_hash_hex, "pending sweep could not be verified")
            }
        }
    }
    Ok(())
}

fn custodian_hex(book: &AddressBook) -> String {
    book.vault.custodian.map(|c| c.to_hex()).unwrap_or_default()
}

// ---------------------------------------------------------------- bitcoin

/// One Bitcoin pass for a custody book: sweep a draining generation when
/// asked to, and report it empty once nothing is left.
pub async fn bitcoin(
    ctx: &OutboundContext<'_>,
    rpc: &dyn BitcoinRpc,
    from: &AddressBook,
    active: Option<&AddressBook>,
    cfg: &BitcoinConfig,
) -> anyhow::Result<()> {
    if !from.vault.is_draining() {
        return Ok(());
    }
    let required = ctx.params.confirmations(Chain::Bitcoin) as u64;
    let key = format!("{}sweep:BTC", from.key_prefix());
    // Every observer, leader or not, attests a confirmed sweep it can see:
    // a coin at the active generation's own address whose inputs were all
    // this generation's. The fee is read off the transaction, so every
    // vote carries the same figure.
    if let (true, Some(active)) = (from.vault.sweep, active) {
        for u in btc::list_unspent(rpc, 1).await? {
            if active.index_of(&u.address) != Some(0)
                || u.confirmations.saturating_sub(1) < required
            {
                continue;
            }
            if ctx
                .state
                .is_submitted(&format!("sweep:{}{}", from.key_prefix(), u.txid_hex))
            {
                continue;
            }
            let Some((ins, fee)) = btc::tx_prevouts(rpc, &u.txid_hex).await? else {
                continue;
            };
            if !ins.iter().all(|(a, _)| from.index_of(a).is_some()) {
                continue;
            }
            let parsed: bitcoin::Txid = u.txid_hex.parse()?;
            let hash: [u8; 32] = <bitcoin::Txid as bitcoin::hashes::Hash>::to_byte_array(parsed);
            report_sweep(
                ctx,
                from,
                "BTC.BTC",
                &u.txid_hex,
                hash,
                u.value_sat as u128,
                fee as u128,
            )
            .await?;
            if record(ctx, &key).is_some_and(|t| t.txid == u.txid_hex) {
                forget(ctx, &key)?;
            }
        }
    }
    if let (true, Some(active)) = (from.vault.sweep, active) {
        verify_pending(ctx, from, |p| async move {
            let mut bytes = hex::decode(&p.tx_hash_hex)?;
            bytes.reverse();
            let display = hex::encode(bytes);
            let Some((ins, fee)) = btc::tx_prevouts(rpc, &display).await? else {
                return Ok(None);
            };
            if !ins.iter().all(|(a, _)| from.index_of(a).is_some()) {
                return Ok(None);
            }
            let confirmed = btc::tx_confirmations(rpc, &display)
                .await?
                .is_some_and(|(_, conf)| conf > required);
            let paid = btc::list_unspent(rpc, 1).await?.iter().any(|u| {
                u.txid_hex == display
                    && active.index_of(&u.address) == Some(0)
                    && u.value_sat as u128 == p.amount
            });
            Ok((confirmed && paid).then_some(fee as u128))
        })
        .await?;
    }
    if let Some(tx) = record(ctx, &key) {
        match btc::tx_confirmations(rpc, &tx.txid).await? {
            Some((_, conf)) if conf > required + 6 => {
                // Confirmed long ago but never seen at the active address:
                // drop the record so the generation is not stuck.
                forget(ctx, &key)?;
            }
            Some(_) => {}
            None => {
                if let Some(raw) = &tx.raw {
                    if !btc::in_mempool(rpc, &tx.txid).await {
                        if let Err(e) = btc::send_raw(rpc, &hex::decode(raw)?).await {
                            tracing::warn!(error = %e, "bitcoin sweep rebroadcast failed");
                        }
                    }
                }
            }
        }
        return Ok(());
    }
    let utxos = btc::vault_utxos(&btc::list_unspent(rpc, 1).await?, from)?;
    if utxos.is_empty() {
        // Nothing left and, when it was to be swept, the sweep is on chain:
        // every observer reports it; the quorum retires it after the grace.
        if !in_flight(ctx, &from.key_prefix()) && (!from.vault.sweep || from.vault.swept_any) {
            report_empty(ctx, from).await?;
        }
        return Ok(());
    }
    let (true, Some(active)) = (from.vault.sweep, active) else {
        return Ok(());
    };
    if !ctx.may_build(&from.vault, move_id(&key)) {
        return Ok(());
    }
    let to = active
        .address(0)
        .ok_or_else(|| anyhow::anyhow!("active generation has no hot address"))?;
    let fee_rate = btc::fee_rate(rpc, cfg.fee_target_blocks, cfg.fallback_sat_per_vb).await?;
    let unsigned = keel_chains::btc::build_sweep(&utxos, to, fee_rate, cfg.network)?;
    let raw_unsigned = hex::encode(bitcoin::consensus::encode::serialize(&unsigned.tx));
    let mut sigs = Vec::with_capacity(unsigned.sighashes.len());
    for s in &unsigned.sighashes {
        let context = SignContext::BtcSweep {
            custodian: custodian_hex(from),
            from_gen: from.vault.gen,
            raw_tx: raw_unsigned.clone(),
            input: s.input as u32,
            prevout_value: unsigned.selected[s.input].value,
            prevout_pubkey: hex::encode(s.pubkey),
            network: cfg.network,
        };
        let sig = ctx
            .tss
            .sign(s.digest, &from.path(s.key_index), Some(&context))
            .await?;
        sigs.push(sig.compact());
    }
    let (raw, txid) = keel_chains::btc::finalize(&unsigned, &sigs)?;
    let txid_hex = <bitcoin::Txid as bitcoin::hashes::Hash>::from_byte_array(txid).to_string();
    let amount = unsigned.tx.output[0].value.to_sat() as u128;
    remember(
        ctx,
        &key,
        Chain::Bitcoin,
        &txid_hex,
        unsigned.fee as u128,
        amount,
        Some(hex::encode(&raw)),
    )?;
    let sent = btc::send_raw(rpc, &raw).await?;
    tracing::info!(txid = %sent, amount, fee = unsigned.fee, gen = from.vault.gen, "bitcoin sweep broadcast");
    Ok(())
}

// ---------------------------------------------------------------- ethereum

fn eth20(addr: &str) -> anyhow::Result<[u8; 20]> {
    Ok(keel_chains::address::eth_decode(addr)?)
}

async fn eth_sign_and_send(
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
    rpc: &dyn EthRpc,
    key: &str,
    tip: u64,
) -> anyhow::Result<Option<(OutboundTx, u128, bool)>> {
    let required = ctx.params.confirmations(Chain::Ethereum) as u64;
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

/// One Ethereum pass for a custody book: consolidate every deposit index
/// with a balance into the hot address (the active generation's when this
/// one is draining with `sweep`), sweep the hot address itself when
/// draining with `sweep`, and report a draining generation empty.
pub async fn ethereum(
    ctx: &OutboundContext<'_>,
    rpc: &dyn EthRpc,
    from: &AddressBook,
    active: Option<&AddressBook>,
    cfg: &EthereumConfig,
) -> anyhow::Result<()> {
    let tip = eth::block_number(rpc).await?;
    let hot = from
        .address(cfg.hot_index)
        .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
        .to_string();
    let sweeping = from.vault.is_draining() && from.vault.sweep && active.is_some();
    let target = match (sweeping, active) {
        (true, Some(a)) => a
            .address(cfg.hot_index)
            .ok_or_else(|| anyhow::anyhow!("active hot address not derived"))?
            .to_string(),
        _ => hot.clone(),
    };
    let prefix = from.key_prefix();
    let custodian = custodian_hex(from);
    let mut found_funds = false;
    let chain_id = eth::chain_id(rpc).await?;
    let (max_fee, priority) = eth::fee_params(rpc, cfg.fallback_priority_wei).await?;

    // Settle what is in flight first.
    let keys: Vec<String> = ctx.state.read(|d| {
        d.moves
            .keys()
            .filter(|k| k.starts_with(&prefix) && k[prefix.len()..].contains(":ETH"))
            .cloned()
            .collect()
    });
    for key in keys {
        if let Some((tx, fee, success)) = eth_settled(ctx, rpc, &key, tip).await? {
            let label = &key[prefix.len()..];
            if success && label.starts_with("sweep:ETH:") {
                let asset = &label["sweep:ETH:".len()..];
                let hash: [u8; 32] = hex::decode(tx.txid.trim_start_matches("0x"))?
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("tx hash length"))?;
                report_sweep(
                    ctx,
                    from,
                    asset,
                    &tx.txid,
                    hash,
                    tx.amount.unwrap_or(0),
                    fee,
                )
                .await?;
            }
            forget(ctx, &key)?;
        } else {
            found_funds = true; // something still moving
        }
    }

    // Sweeps another observer reported: check the transaction on this node
    // (from this generation's hot address to the active one, the amount
    // as reported, deep enough) and vote.
    if let (true, Some(active)) = (sweeping, active) {
        let from_hot = eth20(&hot)?;
        let to_hot = eth20(&target)?;
        let required = ctx.params.confirmations(Chain::Ethereum) as u64;
        let tokens: Vec<(String, [u8; 20])> = cfg
            .tokens
            .iter()
            .filter_map(|t| {
                eth20(&t.contract)
                    .ok()
                    .map(|c| (format!("ETH.{}", t.symbol), c))
            })
            .collect();
        let _ = active;
        verify_pending(ctx, from, |p| {
            let tokens = tokens.clone();
            async move {
                let Some(r) = eth::receipt_status(rpc, &p.tx_hash_hex).await? else {
                    return Ok(None);
                };
                if !r.success || tip.saturating_sub(r.block_number) < required {
                    return Ok(None);
                }
                let Some((f, to, value, input)) =
                    eth::transaction_by_hash(rpc, &p.tx_hash_hex).await?
                else {
                    return Ok(None);
                };
                if f != from_hot {
                    return Ok(None);
                }
                let ok = if p.asset == "ETH.ETH" {
                    to == to_hot && value == p.amount && input.is_empty()
                } else {
                    match tokens.iter().find(|(a, _)| *a == p.asset) {
                        Some((_, contract)) => {
                            to == *contract
                                && input.len() == 68
                                && input[..4] == [0xa9, 0x05, 0x9c, 0xbb]
                                && input[16..36] == to_hot
                                && u128::from_be_bytes(
                                    input[52..68].try_into().unwrap_or([0u8; 16]),
                                ) == p.amount
                                && input[36..52].iter().all(|b| *b == 0)
                        }
                        None => false,
                    }
                };
                Ok(ok.then_some(r.fee_paid))
            }
        })
        .await?;
    }
    // Every observer attests an ERC-20 sweep it can see in the logs: a
    // transfer from this generation's hot address to the active one.
    if let (true, Some(active)) = (sweeping, active) {
        let from_hot = eth20(&hot)?;
        let required = ctx.params.confirmations(Chain::Ethereum) as u64;
        for t in &cfg.tokens {
            let logs = eth::scan_transfers(
                rpc,
                &t.contract,
                active,
                tip.saturating_sub(cfg.log_window),
                tip,
            )
            .await?;
            for l in logs {
                if l.from != from_hot || tip.saturating_sub(l.block_number) < required {
                    continue;
                }
                let txid = format!("0x{}", l.tx_hash_hex.trim_start_matches("0x"));
                if ctx.state.is_submitted(&format!("sweep:{prefix}{txid}")) {
                    continue;
                }
                let fee = eth::receipt_status(rpc, &txid)
                    .await?
                    .map(|r| r.fee_paid)
                    .unwrap_or(0);
                report_sweep(
                    ctx,
                    from,
                    &format!("ETH.{}", t.symbol),
                    &txid,
                    l.tx_hash,
                    l.amount,
                    fee,
                )
                .await?;
            }
        }
    }
    let assets: Vec<(String, Option<String>)> = std::iter::once(("ETH.ETH".to_string(), None))
        .chain(
            cfg.tokens
                .iter()
                .map(|t| (format!("ETH.{}", t.symbol), Some(t.contract.clone()))),
        )
        .collect();
    let gas_cost = (ETH_CONSOLIDATE_GAS as u128).saturating_mul(max_fee);

    // Deposit indexes: fund gas, then move each asset to the target hot.
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
            let key = format!("{prefix}cons:ETH:{asset}:{index}");
            let gas_key = format!("{prefix}gas:ETH:{index}");
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
                    Chain::Ethereum,
                    &txid,
                    tx.max_gas_cost(),
                    value,
                    Some(hex::encode(raw)),
                )?;
                tracing::info!(index, %txid, "ethereum gas funded for consolidation");
                break;
            }
            let nonce = eth::nonce(rpc, &addr).await?;
            let to20 = eth20(&target)?;
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
                sweep: sweeping,
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
            // Into the active generation it is a sweep to attest; into our
            // own hot address it is internal.
            let label = if sweeping {
                format!("sweep:ETH:{asset}")
            } else {
                format!("cons:ETH:{asset}:{index}")
            };
            remember(
                ctx,
                &format!("{prefix}{label}"),
                Chain::Ethereum,
                &txid,
                tx.max_gas_cost(),
                bal,
                Some(hex::encode(raw)),
            )?;
            tracing::info!(index, asset, amount = bal, %txid, sweep = sweeping, "ethereum consolidation broadcast");
        }
    }

    // The hot address itself, when sweeping.
    if sweeping {
        let native = eth::balance(rpc, &hot).await?;
        for (asset, contract) in &assets {
            let bal = match contract {
                Some(c) => eth::token_balance(rpc, c, &hot).await?,
                None => {
                    native.saturating_sub(gas_cost.saturating_mul(1 + cfg.tokens.len() as u128))
                }
            };
            if bal == 0 {
                continue;
            }
            found_funds = true;
            let key = format!("{prefix}sweep:ETH:{asset}");
            if record(ctx, &key).is_some() || !ctx.may_build(&from.vault, move_id(&key)) {
                continue;
            }
            let nonce = eth::nonce(rpc, &hot).await?;
            let to20 = eth20(&target)?;
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
            let context = SignContext::EthSweep {
                custodian: custodian.clone(),
                from_gen: from.vault.gen,
                chain_id,
                nonce,
                max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
                max_fee_per_gas: tx.max_fee_per_gas,
                gas_limit: tx.gas_limit,
                to: format!("0x{}", hex::encode(tx.to)),
                value: tx.value,
                data: hex::encode(&tx.data),
            };
            let (txid, raw) =
                eth_sign_and_send(ctx, rpc, from, cfg.hot_index, &tx, context).await?;
            remember(
                ctx,
                &key,
                Chain::Ethereum,
                &txid,
                tx.max_gas_cost(),
                bal,
                Some(hex::encode(raw)),
            )?;
            tracing::info!(asset, amount = bal, %txid, gen = from.vault.gen, "ethereum sweep broadcast");
        }
    } else if from.vault.is_draining() {
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
    if from.vault.is_draining()
        && !found_funds
        && !in_flight(ctx, &prefix)
        && (!from.vault.sweep || from.vault.swept_any)
    {
        report_empty(ctx, from).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------- tron

async fn tron_sign_and_send(
    ctx: &OutboundContext<'_>,
    api: &dyn TronApi,
    book: &AddressBook,
    index: u64,
    raw: &keel_chains::tron::Raw,
    context: SignContext,
) -> anyhow::Result<(String, serde_json::Value)> {
    let digest = raw.txid();
    let sig = ctx
        .tss
        .sign(digest, &book.path(index), Some(&context))
        .await?;
    let v = keel_chains::eth::recovery_id(&digest, &sig.compact(), &book.pubkey(index)?)?;
    let sig65 = keel_chains::tron::Raw::signature(&sig.compact(), v);
    let json = raw.broadcast_json(&sig65);
    tron::broadcast(api, json.clone()).await?;
    Ok((hex::encode(digest), json))
}

/// One Tron pass for a custody book (see [`ethereum`]).
pub async fn tron(
    ctx: &OutboundContext<'_>,
    api: &dyn TronApi,
    from: &AddressBook,
    active: Option<&AddressBook>,
    cfg: &TronConfig,
) -> anyhow::Result<()> {
    let now = tron::now_block(api).await?;
    let required = ctx.params.confirmations(Chain::Tron) as u64;
    let hot = from
        .address(cfg.hot_index)
        .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
        .to_string();
    let sweeping = from.vault.is_draining() && from.vault.sweep && active.is_some();
    let target = match (sweeping, active) {
        (true, Some(a)) => a
            .address(cfg.hot_index)
            .ok_or_else(|| anyhow::anyhow!("active hot address not derived"))?
            .to_string(),
        _ => hot.clone(),
    };
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

    if sweeping {
        let owner_hot = t21(&hot)?;
        let dest_hot = t21(&target)?;
        let tokens: Vec<(String, [u8; 21])> = cfg
            .tokens
            .iter()
            .filter_map(|t| {
                t21(&t.contract)
                    .ok()
                    .map(|c| (format!("TRON.{}", t.symbol), c))
            })
            .collect();
        let now_number = now.number;
        verify_pending(ctx, from, |p| {
            let tokens = tokens.clone();
            async move {
                let Some(info) = tron::tx_info(api, &p.tx_hash_hex).await? else {
                    return Ok(None);
                };
                if !info.success || now_number.saturating_sub(info.block_number) < required {
                    return Ok(None);
                }
                let Some((owner, dest, amount, native)) =
                    tron::transaction_parts(api, &p.tx_hash_hex).await?
                else {
                    return Ok(None);
                };
                if owner.as_slice() != owner_hot || dest != dest_hot || amount != p.amount {
                    return Ok(None);
                }
                let asset_ok = if native {
                    p.asset == "TRON.TRX"
                } else {
                    // The contract is the trigger's target; the parts give the
                    // recipient and amount, so a matching configured asset is enough.
                    tokens.iter().any(|(a, _)| *a == p.asset)
                };
                Ok(asset_ok.then_some(info.fee_sun as u128))
            }
        })
        .await?;
    }
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
                let label = &key[prefix.len()..];
                if info.success && label.starts_with("sweep:TRON:") {
                    let asset = &label["sweep:TRON:".len()..];
                    let hash: [u8; 32] = hex::decode(&tx.txid)?
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("txid length"))?;
                    report_sweep(
                        ctx,
                        from,
                        asset,
                        &tx.txid,
                        hash,
                        tx.amount.unwrap_or(0),
                        info.fee_sun as u128,
                    )
                    .await?;
                }
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
            let dest = t21(&target)?;
            let raw = match &contract {
                Some(c) => builder.trc20_transfer(&owner, &t21(c)?, &dest, bal),
                None => builder.transfer(&owner, &dest, bal as u64),
            };
            let context = SignContext::TronConsolidate {
                custodian: custodian.clone(),
                index,
                sweep: sweeping,
                raw_data: hex::encode(raw.to_bytes()),
            };
            let (txid, json) = tron_sign_and_send(ctx, api, from, index, &raw, context).await?;
            let label = if sweeping {
                format!("sweep:TRON:{asset}")
            } else {
                format!("cons:TRON:{asset}:{index}")
            };
            remember(
                ctx,
                &format!("{prefix}{label}"),
                Chain::Tron,
                &txid,
                cfg.fee_sun as u128,
                bal,
                Some(json.to_string()),
            )?;
            tracing::info!(index, asset, amount = bal, %txid, sweep = sweeping, "tron consolidation broadcast");
        }
    }

    let (trx, trc20) = tron::account_balances(api, &hot).await?;
    if sweeping {
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
        let keep = (cfg.fee_limit_sun as u128).saturating_mul(1 + tokens.len() as u128);
        if trx > keep {
            moves.push(("TRON.TRX".into(), None, trx - keep));
        }
        for (asset, contract, bal) in moves {
            found_funds = true;
            let key = format!("{prefix}sweep:TRON:{asset}");
            if record(ctx, &key).is_some() || !ctx.may_build(&from.vault, move_id(&key)) {
                continue;
            }
            let owner = t21(&hot)?;
            let dest = t21(&target)?;
            let raw = match &contract {
                Some(c) => builder.trc20_transfer(&owner, &t21(c)?, &dest, bal),
                None => builder.transfer(&owner, &dest, bal as u64),
            };
            let context = SignContext::TronSweep {
                custodian: custodian.clone(),
                from_gen: from.vault.gen,
                raw_data: hex::encode(raw.to_bytes()),
            };
            let (txid, json) =
                tron_sign_and_send(ctx, api, from, cfg.hot_index, &raw, context).await?;
            remember(
                ctx,
                &key,
                Chain::Tron,
                &txid,
                cfg.fee_sun as u128,
                bal,
                Some(json.to_string()),
            )?;
            tracing::info!(asset, amount = bal, %txid, gen = from.vault.gen, "tron sweep broadcast");
        }
    } else if from.vault.is_draining()
        && (trx > cfg.fee_limit_sun as u128 || trc20.values().any(|n| *n > 0))
    {
        found_funds = true;
    }
    if from.vault.is_draining()
        && !found_funds
        && !in_flight(ctx, &prefix)
        && (!from.vault.sweep || from.vault.swept_any)
    {
        report_empty(ctx, from).await?;
    }
    let _ = BTreeSet::<u64>::new();
    Ok(())
}
