//! `ReportNetworkFee`: one report per configured chain per interval. The
//! chain takes the median across observers (`VaultsState::fee_rate`).

use crate::{
    chains::{
        btc::{self, BitcoinRpc},
        eth::{self, EthRpc},
    },
    config::{BitcoinConfig, EthereumConfig, TronConfig},
    rpc::Submitter,
};
use keel_actions::{Action, Chain};

pub async fn report_bitcoin(
    submitter: &Submitter,
    rpc: &dyn BitcoinRpc,
    cfg: &BitcoinConfig,
) -> anyhow::Result<u64> {
    let rate = btc::fee_rate(rpc, cfg.fee_target_blocks, cfg.fallback_sat_per_vb).await?;
    submitter
        .submit(
            None,
            Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: rate,
            },
        )
        .await?;
    Ok(rate)
}

/// Wei per gas: base fee plus the median priority fee.
pub async fn report_ethereum(
    submitter: &Submitter,
    rpc: &dyn EthRpc,
    cfg: &EthereumConfig,
) -> anyhow::Result<u64> {
    let v = rpc
        .call("eth_feeHistory", serde_json::json!(["0x5", "latest", [50]]))
        .await?;
    let (base, tip) = eth::parse_fee_history(&v, cfg.fallback_priority_wei)
        .ok_or_else(|| anyhow::anyhow!("eth_feeHistory"))?;
    let rate = u64::try_from(base.saturating_add(tip)).unwrap_or(u64::MAX);
    submitter
        .submit(
            None,
            Action::ReportNetworkFee {
                chain: Chain::Ethereum,
                fee_rate: rate,
            },
        )
        .await?;
    Ok(rate)
}

/// Tron fees are a configured constant (sun per transfer).
pub async fn report_tron(submitter: &Submitter, cfg: &TronConfig) -> anyhow::Result<u64> {
    submitter
        .submit(
            None,
            Action::ReportNetworkFee {
                chain: Chain::Tron,
                fee_rate: cfg.fee_sun,
            },
        )
        .await?;
    Ok(cfg.fee_sun)
}
