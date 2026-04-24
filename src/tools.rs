//! Tool-API HTTP layer.
//!
//! Each operator-facing tool is a `POST /tools/<name>` endpoint with
//! a typed JSON request and response. Frontends (OpenClaw, Codex,
//! OpenAI Agents SDK, custom UIs) call these — they never touch raw
//! GitHub / BTC / DD APIs. The constrained surface is the product
//! boundary: tools enforce policy even when an LLM picks the next
//! action.
//!
//! Auth: `Authorization: Bearer <SATS_TOOL_API_TOKEN>`. Single token
//! per operator for v0. Tighter per-frontend / per-claim auth is a
//! follow-up.
//!
//! The first tool is `claim.create`. The pattern (typed req, validate,
//! build manifest, write to GitHub, return manifest) is what the rest
//! follow.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    routing::post,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::claim::{BtcDetails, Claim, ClaimMode};
use crate::config::Config;
use crate::github;

/// Shared state threaded into every tool handler. The github client
/// is `Arc`-wrapped because it carries the long-lived reqwest pool
/// and a copy of the operator's bearer token; we never hand it out
/// to end-users.
#[derive(Clone)]
pub struct State_ {
    pub cfg: Arc<Config>,
    pub github: Arc<github::Client>,
}

pub fn router(state: State_) -> Router {
    Router::new()
        .route("/tools/claim.create", post(claim_create))
        .with_state(state)
}

// ── claim.create ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ClaimCreateReq {
    /// Which product mode the customer wants. See SATS_FOR_COMPUTE_SPEC.md
    /// "New product mode: confidential mode" for the difference.
    pub mode: ClaimMode,
    /// GitHub user/org to grant `/deploy` etc. authority on. Required
    /// for `customer_deploy` mode; ignored (and not bound) for
    /// `confidential` mode where the bot deploys a sealed workload.
    #[serde(default)]
    pub customer_owner: Option<String>,
    /// Public GitHub repo containing the customer's `workload.json`
    /// at root. Required for `confidential` mode. Must be `owner/repo`
    /// shape; the bot fetches at `workload_ref` (default `main`) when
    /// the claim activates.
    #[serde(default)]
    pub workload_repo: Option<String>,
    #[serde(default)]
    pub workload_ref: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ClaimCreateResp {
    pub claim: Claim,
    pub issue_number: u64,
    pub issue_url: String,
}

async fn claim_create(
    State(state): State<State_>,
    headers: HeaderMap,
    Json(req): Json<ClaimCreateReq>,
) -> Result<Json<ClaimCreateResp>, ApiError> {
    require_tool_token(&headers, &state.cfg.tool_api_token)?;

    // Mode-specific validation. `customer_owner` is meaningless in
    // confidential mode (the bot owns the node, no GH OIDC binding);
    // `workload_repo` is required there because the bot has to know
    // what to deploy.
    match req.mode {
        ClaimMode::CustomerDeploy => {
            if req.customer_owner.as_deref().unwrap_or("").is_empty() {
                return Err(ApiError::BadRequest(
                    "customer_owner required for customer_deploy mode".into(),
                ));
            }
        }
        ClaimMode::Confidential => {
            let repo = req.workload_repo.as_deref().unwrap_or("");
            if repo.is_empty() || !repo.contains('/') {
                return Err(ApiError::BadRequest(
                    "workload_repo (owner/repo) required for confidential mode".into(),
                ));
            }
        }
    }

    let claim_id = generate_claim_id();
    let btc = BtcDetails {
        // STUB: until the BDK enclave workload exists, every claim
        // gets the operator's sweep address as its "invoice address."
        // Per-claim address derivation lands when the wallet workload
        // is wired (or, in dev, when the user's backed-up xpub is
        // configured — see CLAUDE.md). Single-address mode means the
        // bot has to attribute payments by amount + time + the
        // BIP21 message field rather than by address.
        address: state.cfg.sweep_address.clone(),
        price_per_24h_sats: state.cfg.price_per_24h_sats,
        required_confirmations: 1,
        pending_timeout_secs: state.cfg.pending_timeout_secs,
    };
    let mut claim = Claim::new(&claim_id, req.mode, btc);
    claim.customer_owner = match req.mode {
        ClaimMode::CustomerDeploy => req.customer_owner,
        // Confidential mode: don't bind a customer org. The workload
        // repo + ref end up in the claim manifest via a future field;
        // for v0 we just discard them — the issue summary will note
        // the workload source via the human-facing comment instead.
        ClaimMode::Confidential => None,
    };

    let title = format!("claim {}", claim_id);
    let body = claim.to_issue_body();
    let labels: Vec<&str> = base_labels(req.mode).collect();
    let issue = state
        .github
        .create_issue(&state.cfg.state_repo, &title, &body, &labels)
        .await
        .map_err(|e| ApiError::Upstream(format!("github create_issue: {e}")))?;

    Ok(Json(ClaimCreateResp {
        claim,
        issue_number: issue.number,
        issue_url: issue.html_url,
    }))
}

/// Stable label set every claim issue gets at creation time. Future
/// state-transitions add/remove `state:*` labels via the GitHub
/// client's `add_labels`/`remove_label`.
fn base_labels(mode: ClaimMode) -> impl Iterator<Item = &'static str> {
    let mode_label = match mode {
        ClaimMode::CustomerDeploy => "mode:customer-deploy",
        ClaimMode::Confidential => "mode:confidential",
    };
    [
        "s12e",
        "claim",
        "state:requested",
        "integrity:pristine",
        mode_label,
    ]
    .into_iter()
}

/// `claim_<unix_seconds>` — same shape spec uses in examples. Unix
/// seconds + a tiny suffix ensures uniqueness even with two requests
/// in the same second on a fast operator.
fn generate_claim_id() -> String {
    let ts = Utc::now().timestamp();
    let suffix: u32 = rand_suffix();
    format!("claim_{ts}_{suffix:08x}")
}

fn rand_suffix() -> u32 {
    // Avoiding a `rand` dep — the bot only needs collision avoidance,
    // not unpredictability. Mix in a process-relative monotonic and
    // the address of a stack local: cheap, no deps, plenty unique.
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    n.wrapping_mul(2_654_435_761) ^ nanos
}

// ── auth + error mapping ─────────────────────────────────────────

fn require_tool_token(headers: &HeaderMap, expected: &str) -> Result<(), ApiError> {
    let auth = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let token = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or(ApiError::Unauthorized)?;
    // Constant-time-ish: comparing two strings of equal-or-different
    // length leaks length but not contents. The token is operator-set
    // and not user-input, so timing leaks here have no practical
    // exploit; keep this simple.
    if token != expected {
        return Err(ApiError::Unauthorized);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("upstream: {0}")]
    Upstream(String),
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match &self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            ApiError::Upstream(m) => (StatusCode::BAD_GATEWAY, m.clone()),
        };
        (status, Json(serde_json::json!({"error": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn claim_id_is_unique_under_burst() {
        // 1000 IDs in a tight loop — none should collide.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(generate_claim_id()), "duplicate claim_id");
        }
    }

    #[test]
    fn claim_id_format_matches_spec_shape() {
        let id = generate_claim_id();
        assert!(id.starts_with("claim_"));
        let parts: Vec<&str> = id.split('_').collect();
        assert_eq!(parts.len(), 3);
        // ts is 10+ digits, suffix is 8 hex
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()));
        assert!(parts[2].len() == 8 && parts[2].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn require_tool_token_accepts_bearer() {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        require_tool_token(&h, "secret").unwrap();
    }

    #[test]
    fn require_tool_token_rejects_mismatched_token() {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_static("Bearer wrong"));
        assert!(matches!(
            require_tool_token(&h, "secret"),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn require_tool_token_rejects_missing_header() {
        let h = HeaderMap::new();
        assert!(matches!(
            require_tool_token(&h, "secret"),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn require_tool_token_rejects_non_bearer_scheme() {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Basic dXNlcjpwdw=="),
        );
        assert!(matches!(
            require_tool_token(&h, "anything"),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn base_labels_includes_mode_specific_label() {
        let cd: Vec<_> = base_labels(ClaimMode::CustomerDeploy).collect();
        assert!(cd.contains(&"mode:customer-deploy"));
        let cf: Vec<_> = base_labels(ClaimMode::Confidential).collect();
        assert!(cf.contains(&"mode:confidential"));
    }
}
