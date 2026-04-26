//! BDK-backed test wallet for the signet integration test.
//!
//! Loads a signet descriptor pair from env, syncs from
//! mempool.space's signet esplora, broadcasts payments. The wallet
//! exists so the test is fully self-driving — no manual faucet drip
//! between runs once the seed is funded once.
//!
//! One-time setup (per operator):
//!
//! 1. Generate a signet wallet (Sparrow → New Wallet → Signet, or
//!    `bitcoin-cli createwallet` against a signet node, or `bdk-cli`).
//! 2. Export the external + change descriptors (e.g.
//!    `wpkh(tprv8.../84h/1h/0h/0/*)` and the `/1/*` variant).
//! 3. Hit a signet faucet for the wallet's first receive address with
//!    enough sats to cover many test runs (e.g. 1M sats).
//! 4. Set the descriptors as repo secrets:
//!    `SATS_TEST_SIGNET_DESCRIPTOR` and
//!    `SATS_TEST_SIGNET_CHANGE_DESCRIPTOR`.

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use bdk_esplora::{EsploraAsyncExt, esplora_client};
use bdk_wallet::{
    KeychainKind, SignOptions, Wallet,
    bitcoin::{Address, Amount, Network, Txid},
};

const DEFAULT_SIGNET_ESPLORA: &str = "https://mempool.space/signet/api";
const SCAN_PARALLEL_REQS: usize = 5;
/// Stop-gap address-search count for full-scan. BDK scans this many
/// unused addresses past the last-funded one before declaring the
/// chain exhausted. 5 is plenty for a tiny test wallet that only ever
/// touches index 0.
const SCAN_STOP_GAP: usize = 5;

pub struct SignetWallet {
    wallet: Wallet,
    client: esplora_client::AsyncClient,
}

impl SignetWallet {
    /// Build the wallet from env vars and run a full scan against
    /// mempool.space's signet esplora. Returns early with a clear
    /// `skip` instruction if the descriptor env vars aren't set —
    /// callers are gated tests, the env-missing path should be a
    /// soft skip not a hard fail.
    pub async fn from_env_or_skip() -> Result<Option<Self>> {
        let Some(desc) = read_env("SATS_TEST_SIGNET_DESCRIPTOR")? else {
            eprintln!("integ: skipping — SATS_TEST_SIGNET_DESCRIPTOR not set");
            return Ok(None);
        };
        let Some(change) = read_env("SATS_TEST_SIGNET_CHANGE_DESCRIPTOR")? else {
            eprintln!("integ: skipping — SATS_TEST_SIGNET_CHANGE_DESCRIPTOR not set");
            return Ok(None);
        };
        let esplora_url = std::env::var("SATS_TEST_SIGNET_ESPLORA")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_SIGNET_ESPLORA.into());

        let mut wallet = Wallet::create(desc, change)
            .network(Network::Signet)
            .create_wallet_no_persist()
            .context("Wallet::create")?;

        let client = esplora_client::Builder::new(&esplora_url)
            .build_async()
            .context("esplora_client::Builder::build_async")?;

        let scan = wallet.start_full_scan().build();
        let update = client
            .full_scan(scan, SCAN_STOP_GAP, SCAN_PARALLEL_REQS)
            .await
            .context("esplora full_scan")?;
        wallet.apply_update(update).context("wallet.apply_update")?;

        Ok(Some(Self { wallet, client }))
    }

    /// Derive a fresh receive address for the bot to use as
    /// `SATS_SWEEP_ADDRESS`. Each test run gets a new address; the
    /// wallet still spends from any of its UTXOs regardless of which
    /// receive address it's deriving.
    pub fn next_unused_address(&mut self, keychain: KeychainKind) -> bdk_wallet::bitcoin::Address {
        self.wallet.next_unused_address(keychain).address
    }

    /// Confirmed + trusted-pending balance, in sats.
    pub fn balance_sats(&self) -> u64 {
        let b = self.wallet.balance();
        // `total()` includes confirmed + trusted_pending + immature
        // + untrusted_pending. For a self-broadcasting test wallet
        // we want spendable = confirmed + trusted_pending.
        (b.confirmed + b.trusted_pending).to_sat()
    }

    /// Bail with a funding-instructions message if the wallet has no
    /// spendable sats. The first address is the one the operator
    /// should fund from a signet faucet.
    pub fn ensure_funded(&mut self, min_sats: u64) -> Result<()> {
        let bal = self.balance_sats();
        if bal >= min_sats {
            return Ok(());
        }
        let fund_addr = self.wallet.next_unused_address(KeychainKind::External);
        bail!(
            "test wallet has {bal} sats (< {min_sats} needed). \
             Fund this address from a signet faucet, then re-run: {fund_addr}"
        );
    }

    /// Broadcast a tx paying exactly `sats` to `target_addr`. Fee is
    /// chosen by BDK's default coin-selection. Returns the txid once
    /// the esplora client accepts the broadcast.
    pub async fn broadcast_exact(&mut self, target_addr: &str, sats: u64) -> Result<Txid> {
        let target = Address::from_str(target_addr)
            .with_context(|| format!("parse target address {target_addr:?}"))?
            .require_network(Network::Signet)
            .with_context(|| format!("address {target_addr:?} not on signet"))?;

        let mut builder = self.wallet.build_tx();
        builder.add_recipient(target.script_pubkey(), Amount::from_sat(sats));
        let mut psbt = builder.finish().context("build_tx.finish")?;
        let finalized = self
            .wallet
            .sign(&mut psbt, SignOptions::default())
            .context("wallet.sign")?;
        if !finalized {
            bail!("wallet.sign returned non-finalized PSBT");
        }
        let tx = psbt.extract_tx().context("psbt.extract_tx")?;
        let txid = tx.compute_txid();
        self.client
            .broadcast(&tx)
            .await
            .with_context(|| format!("esplora broadcast {txid}"))?;
        Ok(txid)
    }
}

fn read_env(key: &str) -> Result<Option<String>> {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => Ok(Some(v)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(anyhow!("read env {key}: {e}")),
    }
}
