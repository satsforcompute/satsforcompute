//! Bitcoin payment watcher.
//!
//! [`MempoolSpace`] is the only [`BtcWatcher`] impl: it talks to
//! `https://mempool.space` REST against mainnet or signet. There is
//! no regtest / mock backend — integration tests run against real
//! signet via the same adapter.
//!
//! What the bot needs from BTC:
//!
//! - "List all txs that paid this address" (confirmed + mempool) →
//!   [`BtcWatcher::list_address_txs`].
//! - "What's the chain tip?" → [`BtcWatcher::current_block_height`].
//!   Pair with [`AddressTx::confirmations`] to count confs.
//!
//! RBF / double-spend detection is out of scope: the bot treats the
//! mempool-seen → 1-conf path as a one-way ratchet and reconciles
//! lost payments via the manual-review refund flow.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;

const DEFAULT_MEMPOOL_BASE_URL: &str = "https://mempool.space/api";

/// One transaction observed paying into a tracked address.
#[derive(Debug, Clone)]
pub struct AddressTx {
    pub txid: String,
    /// Sats received at the watched address by this tx.
    pub received_sats: u64,
    /// `Some(height)` if confirmed; `None` while still in the mempool.
    pub block_height: Option<u64>,
    /// Unix seconds the block was mined. `None` while unconfirmed.
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

#[async_trait]
pub trait BtcWatcher: Send + Sync {
    async fn list_address_txs(&self, address: &str) -> Result<Vec<AddressTx>>;
    async fn current_block_height(&self) -> Result<u64>;
}

// ── mempool.space adapter ─────────────────────────────────────────

#[derive(Clone)]
pub struct MempoolSpace {
    http: reqwest::Client,
    base_url: String,
}

impl MempoolSpace {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_MEMPOOL_BASE_URL)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }
}

impl Default for MempoolSpace {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BtcWatcher for MempoolSpace {
    async fn list_address_txs(&self, address: &str) -> Result<Vec<AddressTx>> {
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
        let raw: Vec<MempoolRawTx> = resp.json().await.with_context(|| format!("parse {url}"))?;
        Ok(raw
            .into_iter()
            .map(|tx| project_mempool_tx(tx, address))
            .collect())
    }

    async fn current_block_height(&self) -> Result<u64> {
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

#[derive(Debug, Deserialize)]
struct MempoolRawTx {
    txid: String,
    #[serde(default)]
    vout: Vec<MempoolRawVout>,
    status: MempoolRawStatus,
}

#[derive(Debug, Deserialize)]
struct MempoolRawVout {
    #[serde(default)]
    scriptpubkey_address: Option<String>,
    value: u64,
}

#[derive(Debug, Deserialize)]
struct MempoolRawStatus {
    confirmed: bool,
    #[serde(default)]
    block_height: Option<u64>,
    #[serde(default)]
    block_time: Option<u64>,
}

fn project_mempool_tx(raw: MempoolRawTx, target_address: &str) -> AddressTx {
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

    fn fixture_two_outputs_one_match() -> MempoolRawTx {
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
        let tx = project_mempool_tx(raw, "bc1qme");
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
        let raw: MempoolRawTx = serde_json::from_str(json).unwrap();
        let tx = project_mempool_tx(raw, "bc1qme");
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
        assert_eq!(tx.confirmations(800_000), 0);
    }

    #[test]
    fn project_ignores_outputs_with_no_address() {
        let json = r#"{
            "txid": "with_op_return",
            "vin": [],
            "vout": [
                { "value": 0 },
                { "scriptpubkey_address": "bc1qme", "value": 50000 }
            ],
            "status": { "confirmed": false }
        }"#;
        let raw: MempoolRawTx = serde_json::from_str(json).unwrap();
        let tx = project_mempool_tx(raw, "bc1qme");
        assert_eq!(tx.received_sats, 50_000);
    }
}
