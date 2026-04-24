//! Bitcoin payment watcher.
//!
//! v0 ships a single [`MempoolSpace`] adapter (https://mempool.space).
//! Bitcoin Core RPC + Electrum + self-hosted mempool.space land later.
//!
//! Pluggability is achieved by swapping the concrete struct held in
//! the bot's axum state; we don't pay the trait-object tax until
//! there's a second implementation. The methods below are stable
//! enough that a future trait extraction will be mechanical.
//!
//! What the bot needs:
//!
//! - "List all txs that paid this claim's address" → [`MempoolSpace::
//!   list_address_txs`]. Used by the per-claim payment poll.
//! - "How many confirmations does this tx have?" → derived from the
//!   tx's block height + the chain tip ([`MempoolSpace::current_block_height`]).
//!
//! RBF / double-spend detection is a follow-up; v0 treats the
//! mempool-seen → 1-conf path as a one-way ratchet and reconciles via
//! the manual-review refund flow if a tx never confirms.
//!
//! Spec: SATS_FOR_COMPUTE_SPEC.md "BTC Watcher" section.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://mempool.space/api";

/// One transaction observed paying into a tracked address.
#[derive(Debug, Clone)]
pub struct AddressTx {
    pub txid: String,
    /// Sats received at the watched address by this tx (sum of all
    /// outputs whose `scriptpubkey_address` matches). Inputs spent
    /// from the address are not subtracted — the bot only watches
    /// addresses that belong to its own derived chain, so spends are
    /// the bot's own sweeps and don't count as customer payments.
    pub received_sats: u64,
    /// `Some(height)` if the tx is confirmed; `None` while it's still
    /// in the mempool. Pair with [`MempoolSpace::current_block_height`]
    /// to compute confirmation count.
    pub block_height: Option<u64>,
    /// Unix seconds at which the block including this tx was mined.
    /// `None` while unconfirmed.
    pub block_time: Option<u64>,
}

impl AddressTx {
    /// Confirmations given a chain tip. 0 if still in mempool.
    pub fn confirmations(&self, tip: u64) -> u32 {
        match self.block_height {
            Some(h) if tip >= h => (tip - h + 1) as u32,
            _ => 0,
        }
    }
}

#[derive(Clone)]
pub struct MempoolSpace {
    http: reqwest::Client,
    base_url: String,
}

impl MempoolSpace {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// `GET /api/address/{address}/txs` — returns up to 50 mempool
    /// txs plus the first 25 confirmed txs touching the address.
    /// More than enough for a per-claim address that only sees one
    /// (or a small number of top-up) payments.
    pub async fn list_address_txs(&self, address: &str) -> Result<Vec<AddressTx>> {
        let url = format!(
            "{}/address/{}/txs",
            self.base_url.trim_end_matches('/'),
            address
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("GET {url} → {s}: {body}");
        }
        let raw: Vec<RawTx> = resp.json().await.with_context(|| format!("parse {url}"))?;
        Ok(raw.into_iter().map(|tx| project_tx(tx, address)).collect())
    }

    /// `GET /api/blocks/tip/height` — current chain-tip height. Pair
    /// with [`AddressTx::confirmations`].
    pub async fn current_block_height(&self) -> Result<u64> {
        let url = format!("{}/blocks/tip/height", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("GET {url} → {s}: {body}");
        }
        let body = resp.text().await?;
        body.trim()
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("parse tip height {body:?}: {e}"))
    }
}

impl Default for MempoolSpace {
    fn default() -> Self {
        Self::new()
    }
}

// ── Wire types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RawTx {
    txid: String,
    #[serde(default)]
    vout: Vec<RawVout>,
    status: RawStatus,
}

#[derive(Debug, Deserialize)]
struct RawVout {
    #[serde(default)]
    scriptpubkey_address: Option<String>,
    value: u64,
}

#[derive(Debug, Deserialize)]
struct RawStatus {
    confirmed: bool,
    #[serde(default)]
    block_height: Option<u64>,
    #[serde(default)]
    block_time: Option<u64>,
}

fn project_tx(raw: RawTx, target_address: &str) -> AddressTx {
    let received_sats: u64 = raw
        .vout
        .iter()
        .filter(|v| v.scriptpubkey_address.as_deref() == Some(target_address))
        .map(|v| v.value)
        .sum();
    AddressTx {
        txid: raw.txid,
        received_sats,
        block_height: if raw.status.confirmed {
            raw.status.block_height
        } else {
            None
        },
        block_time: raw.status.block_time,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_two_outputs_one_match() -> RawTx {
        // Synthesized after the public mempool.space response shape
        // — kept in code so the test doesn't need a network round-trip.
        let json = r#"{
            "txid": "abc123",
            "version": 2,
            "vin": [],
            "vout": [
                { "scriptpubkey_address": "bc1qother", "value": 12345 },
                { "scriptpubkey_address": "bc1qme", "value": 50000 },
                { "scriptpubkey_address": "bc1qme", "value": 1000 }
            ],
            "status": {
                "confirmed": true,
                "block_height": 850123,
                "block_time": 1737459123
            }
        }"#;
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn project_sums_only_matching_outputs() {
        let raw = fixture_two_outputs_one_match();
        let tx = project_tx(raw, "bc1qme");
        assert_eq!(tx.txid, "abc123");
        assert_eq!(tx.received_sats, 51_000);
        assert_eq!(tx.block_height, Some(850123));
        assert_eq!(tx.block_time, Some(1737459123));
    }

    #[test]
    fn project_unconfirmed_has_no_block_height() {
        let json = r#"{
            "txid": "pending1",
            "vin": [],
            "vout": [{ "scriptpubkey_address": "bc1qme", "value": 50000 }],
            "status": { "confirmed": false }
        }"#;
        let raw: RawTx = serde_json::from_str(json).unwrap();
        let tx = project_tx(raw, "bc1qme");
        assert_eq!(tx.block_height, None);
        assert_eq!(tx.confirmations(900_000), 0);
    }

    #[test]
    fn confirmations_from_tip_and_height() {
        let tx = AddressTx {
            txid: "x".into(),
            received_sats: 50_000,
            block_height: Some(850_000),
            block_time: None,
        };
        assert_eq!(tx.confirmations(850_000), 1);
        assert_eq!(tx.confirmations(850_005), 6);
        // tip < block_height should never happen in practice; treat
        // as 0 rather than panic on subtraction.
        assert_eq!(tx.confirmations(800_000), 0);
    }

    #[test]
    fn project_ignores_outputs_with_no_address() {
        // OP_RETURN outputs have no scriptpubkey_address — must skip
        // them rather than count as 0-sats matching.
        let json = r#"{
            "txid": "with_op_return",
            "vin": [],
            "vout": [
                { "value": 0 },
                { "scriptpubkey_address": "bc1qme", "value": 50000 }
            ],
            "status": { "confirmed": false }
        }"#;
        let raw: RawTx = serde_json::from_str(json).unwrap();
        let tx = project_tx(raw, "bc1qme");
        assert_eq!(tx.received_sats, 50_000);
    }
}
