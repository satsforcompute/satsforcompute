//! Shared harness for integration tests.
//!
//! Spawns the built `satsforcompute` binary on an ephemeral port, waits
//! for `/healthz`, and exposes a thin client over the tool API. Drops
//! kill the subprocess. No mocks — the bot is the same binary we ship.
//! `signet_wallet` is the BDK side-channel that broadcasts a real
//! signet payment so the test is fully self-driving.

pub mod signet_wallet;

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use serde_json::Value;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// A live `satsforcompute` subprocess. Drop kills the child.
pub struct BotHarness {
    child: Child,
    pub base_url: String,
    pub tool_token: String,
}

impl Drop for BotHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl BotHarness {
    /// Spawn the bot. Caller passes the env vars the bot reads —
    /// `SATS_STATE_REPO`, `SATS_OPS_REPO`, `SATS_SWEEP_ADDRESS`,
    /// `SATS_GITHUB_TOKEN`, `SATS_TOOL_API_TOKEN`, and
    /// `SATS_MEMPOOL_BASE_URL`.
    pub async fn spawn(extra_env: Vec<(String, String)>) -> Result<Self> {
        let bin = locate_binary()?;
        let port = pick_free_port()?;
        let tool_token = "integ-test-token".to_string();

        let mut cmd = Command::new(&bin);
        cmd.env("SATS_PORT", port.to_string())
            .env("SATS_TOOL_API_TOKEN", &tool_token)
            .env("RUST_LOG", "satsforcompute=debug")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        let child = cmd
            .spawn()
            .with_context(|| format!("spawn {}", bin.display()))?;
        let base_url = format!("http://127.0.0.1:{port}");
        let mut h = BotHarness {
            child,
            base_url,
            tool_token,
        };
        h.wait_for_healthz().await?;
        Ok(h)
    }

    async fn wait_for_healthz(&mut self) -> Result<()> {
        let url = format!("{}/healthz", self.base_url);
        let client = reqwest::Client::new();
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait()? {
                bail!("bot exited before /healthz came up: {status}");
            }
            if let Ok(resp) = client
                .get(&url)
                .timeout(Duration::from_secs(1))
                .send()
                .await
                && resp.status() == StatusCode::OK
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("/healthz did not come up in {:?}", STARTUP_TIMEOUT);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// POST to a `/tools/<tool>` endpoint with bearer auth. Panics on
    /// non-2xx responses (the test wants to fail loud on misconfig).
    pub async fn tool(&self, tool: &str, body: Value) -> Result<Value> {
        let url = format!("{}/tools/{}", self.base_url, tool);
        let resp = reqwest::Client::new()
            .post(&url)
            .bearer_auth(&self.tool_token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("POST {url} → {status}: {text}");
        }
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("parse JSON from {url}: {text}"))?;
        Ok(v)
    }
}

fn locate_binary() -> Result<PathBuf> {
    // CARGO_BIN_EXE_<name> is set by cargo for integration tests. It
    // points at the freshly-built binary in target/<profile>/.
    if let Some(p) = option_env!("CARGO_BIN_EXE_satsforcompute") {
        return Ok(PathBuf::from(p));
    }
    bail!("CARGO_BIN_EXE_satsforcompute not set — run via `cargo test`")
}

fn pick_free_port() -> Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0").context("bind ephemeral")?;
    Ok(l.local_addr()?.port())
}

// ── env helpers ───────────────────────────────────────────────────

/// Read a required env var or skip the test with a clear message.
/// Use in `#[ignore]` integration tests so a missing env doesn't
/// silently green the run.
#[macro_export]
macro_rules! require_env_or_skip {
    ($key:expr) => {
        match std::env::var($key) {
            Ok(v) if !v.is_empty() => v,
            _ => {
                eprintln!("integ: skipping — {} not set", $key);
                return Ok(());
            }
        }
    };
}
