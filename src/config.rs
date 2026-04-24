//! Environment-derived configuration.
//!
//! All operator-tunable values live here. Forking operators set these
//! at deploy time (typically as GitHub Actions secrets baked into the
//! enclave's config disk). No runtime mutation.
//!
//! See `SATS_FOR_COMPUTE_SPEC.md` for the canonical name of each knob.

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Config {
    /// HTTP listen port. Default 8080 to match the dd-agent default
    /// so the operator workload can reuse the same `expose` shape.
    pub port: u16,
    /// State repo for claim issues, e.g. `myorg/s4c-ops`. Public
    /// for demo, private for production. Configurable so forking
    /// operators don't share state.
    pub state_repo: String,
    /// Optional code repo (public). Just for self-reference in
    /// claim-comment links.
    pub code_repo: String,
    /// DevOpsDefender control-plane URL the operator uses to boot
    /// new agents and dispatch /owner workflows on. Defaults to
    /// the canonical `app.devopsdefender.com`.
    pub dd_cp_url: String,
    /// Operator's BTC sweep address (cold storage). The `wallet.
    /// sweep_cold` tool only sends here. Public addresses are not
    /// secrets in the crypto sense, but routing through a GitHub
    /// secret keeps the address out of the repo + CI logs and makes
    /// rotation trivial when the exchange cycles a deposit address.
    pub sweep_address: String,
    /// Price per 24-hour claim block, in sats. Spec ships with
    /// 50,000 (~$30/day). Operator-configurable.
    pub price_per_24h_sats: u64,
    /// Time the optimistic 0-conf activation stays "pending" before
    /// the bot gives up and reclaims the node. Spec recommendation:
    /// 3 hours (vs the original 1h, which fee-market congestion
    /// false-terminates).
    pub pending_timeout_secs: u64,
    /// GitHub bearer token. Either a fine-grained PAT (quickstart) or
    /// a GitHub App installation token (production). Provides
    /// read+write on the configured `state_repo`'s issues, comments,
    /// and labels — see `github.rs` for the exact REST surface used.
    /// Held only in process memory; never logged.
    pub github_token: String,
    /// Shared bearer token gating the `/tools/*` API. Operator
    /// frontends (OpenClaw, custom UI, etc.) present it as
    /// `Authorization: Bearer <token>`. Single token per operator
    /// for v0; multi-tenant tool-API auth (per-frontend tokens) is a
    /// future cleanup.
    pub tool_api_token: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let port = parse_env("SATS_PORT", "8080")?;
        let state_repo = require_env("SATS_STATE_REPO")?;
        let code_repo = std::env::var("SATS_CODE_REPO")
            .unwrap_or_else(|_| "satsforcompute/satsforcompute".into());
        let dd_cp_url = std::env::var("SATS_DD_CP_URL")
            .unwrap_or_else(|_| "https://app.devopsdefender.com".into());
        let sweep_address = require_env("SATS_SWEEP_ADDRESS")?;
        let price_per_24h_sats = parse_env("SATS_PRICE_PER_24H_SATS", "50000")?;
        let pending_timeout_secs = parse_env("SATS_PENDING_TIMEOUT_SECS", "10800")?;
        let github_token = require_env("SATS_GITHUB_TOKEN")?;
        let tool_api_token = require_env("SATS_TOOL_API_TOKEN")?;

        if !state_repo.contains('/') {
            bail!("SATS_STATE_REPO must be 'owner/repo', got {state_repo:?}");
        }
        if sweep_address.is_empty() {
            bail!("SATS_SWEEP_ADDRESS must be set to a BTC address");
        }

        Ok(Self {
            port,
            state_repo,
            code_repo,
            dd_cp_url,
            sweep_address,
            price_per_24h_sats,
            pending_timeout_secs,
            github_token,
            tool_api_token,
        })
    }
}

fn require_env(key: &str) -> Result<String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .with_context(|| format!("{key} must be set"))
}

fn parse_env<T>(key: &str, default: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let raw = std::env::var(key).unwrap_or_else(|_| default.into());
    raw.parse().map_err(|e| anyhow::anyhow!("{key}={raw}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_falls_back_to_default() {
        // Not setting the var; default kicks in.
        // We don't synchronize env access here — `SATS_PORT_TEST_X`
        // is unique per test name, so cross-test contention is nil.
        let v: u16 = parse_env("SATS_PORT_TEST_FALLBACK", "9999").unwrap();
        assert_eq!(v, 9999);
    }

    #[test]
    fn require_env_errors_on_unset() {
        let r = require_env("SATS_REQUIRED_TEST_UNSET");
        assert!(r.is_err());
    }
}
