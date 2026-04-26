//! Environment-derived configuration.
//!
//! Operator-tunable values, set at deploy time. No runtime mutation.
//! See `SATS_FOR_COMPUTE_SPEC.md` for the canonical name of each knob.

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Config {
    /// HTTP listen port. Default 8090 (NOT 8080: dd-agent itself
    /// listens on localhost:8080 inside the host, and the workload's
    /// `expose` ingress would otherwise route `bot.<agent>` traffic to
    /// dd-agent instead of the bot).
    pub port: u16,
    /// State repo for claim issues, e.g. `myorg/s4c-ops`.
    pub state_repo: String,
    /// Operator-ops repo holding privileged workflows the bot fires
    /// via `workflow_dispatch`. The ops repo is what actually does
    /// cloud provisioning + DD `/owner` calls; the bot itself never
    /// holds those creds. See `OPS_REPO.md` for the workflow contract.
    pub ops_repo: String,
    /// Filename of the boot-agent workflow inside `ops_repo`.
    /// Defaults to `boot-agent.yml`.
    pub ops_boot_workflow: String,
    /// Filename of the owner-update workflow inside `ops_repo`.
    /// Defaults to `owner-update.yml`.
    pub ops_owner_workflow: String,
    /// Git ref the dispatched workflows run from. Default `main`.
    pub ops_ref: String,
    /// DevOpsDefender control-plane URL. Default `app.devopsdefender.com`.
    pub dd_cp_url: String,
    /// Operator's BTC sweep / invoice address. v0 stub: every claim
    /// invoices into this single address. Per-claim derivation lands
    /// with the BDK enclave wallet.
    pub sweep_address: String,
    /// Price per 24-hour claim block, in sats. Default 50,000.
    pub price_per_24h_sats: u64,
    /// Pending-payment grace window in seconds. Carried on each
    /// claim's `BtcDetails` per spec `s12e.claim.v1`. Default 10800.
    pub pending_timeout_secs: u64,
    /// GitHub bearer token (PAT or app installation token). Read+write
    /// on `state_repo` issues + `ops_repo` dispatches.
    pub github_token: String,
    /// Shared bearer token gating `/tools/*`.
    pub tool_api_token: String,
    /// Optional bearer token the bot presents to dd-agent's `/health`
    /// when polling for `agent_owner` flip. Cloudflare zero-trust
    /// fronts dd-agents in production, so a service token may be
    /// required. Empty / unset = no auth header.
    pub dd_auth_token: Option<String>,
    /// `mempool.space` REST base URL. Defaults to mainnet
    /// (`https://mempool.space/api`); set `SATS_MEMPOOL_BASE_URL` to
    /// e.g. `https://mempool.space/signet/api` for the signet
    /// integration test.
    pub mempool_base_url: String,
    /// How long to wait after an optimistic 0-conf
    /// `dd.dispatch_owner_update` before reaping the claim if the
    /// underlying tx still hasn't reached `required_confirmations`.
    /// Default 3600s (1h). On reap, the bot dispatches owner-update
    /// with an empty `agent_owner` to revoke access and transitions
    /// the claim to `failed`.
    pub optimistic_bind_grace_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let port = parse_env("SATS_PORT", "8090")?;
        let state_repo = require_env("SATS_STATE_REPO")?;
        let ops_repo = require_env("SATS_OPS_REPO")?;
        let ops_boot_workflow =
            std::env::var("SATS_OPS_BOOT_WORKFLOW").unwrap_or_else(|_| "boot-agent.yml".into());
        let ops_owner_workflow =
            std::env::var("SATS_OPS_OWNER_WORKFLOW").unwrap_or_else(|_| "owner-update.yml".into());
        let ops_ref = std::env::var("SATS_OPS_REF").unwrap_or_else(|_| "main".into());
        let dd_cp_url = std::env::var("SATS_DD_CP_URL")
            .unwrap_or_else(|_| "https://app.devopsdefender.com".into());
        let sweep_address = require_env("SATS_SWEEP_ADDRESS")?;
        let price_per_24h_sats = parse_env("SATS_PRICE_PER_24H_SATS", "50000")?;
        let pending_timeout_secs = parse_env("SATS_PENDING_TIMEOUT_SECS", "10800")?;
        let github_token = require_env("SATS_GITHUB_TOKEN")?;
        let tool_api_token = require_env("SATS_TOOL_API_TOKEN")?;
        let dd_auth_token = std::env::var("SATS_DD_AUTH_TOKEN")
            .ok()
            .filter(|v| !v.is_empty());
        let mempool_base_url = std::env::var("SATS_MEMPOOL_BASE_URL")
            .unwrap_or_else(|_| "https://mempool.space/api".into());
        let optimistic_bind_grace_secs = parse_env("SATS_OPTIMISTIC_BIND_GRACE_SECS", "3600")?;

        if !state_repo.contains('/') {
            bail!("SATS_STATE_REPO must be 'owner/repo', got {state_repo:?}");
        }
        if !ops_repo.contains('/') {
            bail!("SATS_OPS_REPO must be 'owner/repo', got {ops_repo:?}");
        }
        if sweep_address.is_empty() {
            bail!("SATS_SWEEP_ADDRESS must be set to a BTC address");
        }

        Ok(Self {
            port,
            state_repo,
            ops_repo,
            ops_boot_workflow,
            ops_owner_workflow,
            ops_ref,
            dd_cp_url,
            sweep_address,
            price_per_24h_sats,
            pending_timeout_secs,
            github_token,
            tool_api_token,
            dd_auth_token,
            mempool_base_url,
            optimistic_bind_grace_secs,
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
        let v: u16 = parse_env("SATS_PORT_TEST_FALLBACK", "9999").unwrap();
        assert_eq!(v, 9999);
    }

    #[test]
    fn require_env_errors_on_unset() {
        let r = require_env("SATS_REQUIRED_TEST_UNSET");
        assert!(r.is_err());
    }
}
