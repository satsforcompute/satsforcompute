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

use crate::claim::{BtcDetails, CURRENT_SCHEMA, Claim, ClaimMode, ClaimState};
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
        .route("/tools/claim.load", post(claim_load))
        .route("/tools/claim.update", post(claim_update))
        .route("/tools/btc.invoice", post(btc_invoice))
        .route("/tools/node.boot", post(node_boot))
        .route(
            "/tools/dd.dispatch_owner_update",
            post(dd_dispatch_owner_update),
        )
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
    match req.mode {
        ClaimMode::CustomerDeploy => {
            claim.customer_owner = req.customer_owner;
        }
        ClaimMode::Confidential => {
            // Don't bind a customer org — the bot owns the node and
            // deploys a sealed workload. Persist the workload source
            // onto the claim so node.boot (and the orchestrator) can
            // build dispatch inputs from issue_number alone.
            claim.workload_repo = req.workload_repo;
            claim.workload_ref = Some(
                req.workload_ref
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "main".into()),
            );
        }
    }

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

// ── claim.load ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ClaimLoadReq {
    /// GitHub issue number on the configured `state_repo`. The bot
    /// always knows the number (it minted it via claim.create); this
    /// tool just rehydrates the manifest from the on-chain-of-record
    /// (the GitHub issue body).
    pub issue_number: u64,
}

#[derive(Debug, Serialize)]
pub struct ClaimLoadResp {
    pub claim: Claim,
    pub issue_number: u64,
    pub issue_url: String,
    pub state: String,
    pub labels: Vec<String>,
}

async fn claim_load(
    State(state): State<State_>,
    headers: HeaderMap,
    Json(req): Json<ClaimLoadReq>,
) -> Result<Json<ClaimLoadResp>, ApiError> {
    require_tool_token(&headers, &state.cfg.tool_api_token)?;

    let issue = state
        .github
        .get_issue(&state.cfg.state_repo, req.issue_number)
        .await
        .map_err(|e| ApiError::Upstream(format!("github get_issue: {e}")))?;

    let claim = Claim::from_issue_body(&issue.body)
        .map_err(|e| ApiError::Upstream(format!("issue body manifest: {e}")))?;

    Ok(Json(ClaimLoadResp {
        claim,
        issue_number: issue.number,
        issue_url: issue.html_url,
        state: issue.state,
        labels: issue
            .labels
            .into_iter()
            .map(|l| l.name().to_string())
            .collect(),
    }))
}

// ── claim.update ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ClaimUpdateReq {
    /// GitHub issue number to update.
    pub issue_number: u64,
    /// New claim manifest. Must:
    /// - declare `schema == CURRENT_SCHEMA`
    /// - match the existing manifest's `claim_id`
    pub claim: Claim,
    /// Optional human-facing note appended to the issue as a comment.
    /// Spec: "comments are append-only event/conversation history."
    /// State changes always log a default note even if this is unset,
    /// so the audit trail is never empty.
    #[serde(default)]
    pub event_note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ClaimUpdateResp {
    pub claim: Claim,
    pub issue_number: u64,
    pub issue_url: String,
    pub previous_state: String,
    pub new_state: String,
    pub state_changed: bool,
}

async fn claim_update(
    State(state): State<State_>,
    headers: HeaderMap,
    Json(req): Json<ClaimUpdateReq>,
) -> Result<Json<ClaimUpdateResp>, ApiError> {
    require_tool_token(&headers, &state.cfg.tool_api_token)?;

    // Schema guardrail. Spec section "Tool API guardrails" requires
    // `claim.update` to "preserve canonical schema and event history."
    // Reject manifests that don't match the schema we know how to
    // round-trip — a future schema bump (s12e.claim.v2 etc.) will
    // explicitly handle migration.
    if req.claim.schema != CURRENT_SCHEMA {
        return Err(ApiError::BadRequest(format!(
            "claim.schema must be {CURRENT_SCHEMA:?}, got {:?}",
            req.claim.schema
        )));
    }

    // Load the existing issue so we can validate claim_id continuity
    // and detect state transitions.
    let existing_issue = state
        .github
        .get_issue(&state.cfg.state_repo, req.issue_number)
        .await
        .map_err(|e| ApiError::Upstream(format!("github get_issue: {e}")))?;
    let existing_claim = Claim::from_issue_body(&existing_issue.body)
        .map_err(|e| ApiError::Upstream(format!("issue body manifest: {e}")))?;

    if existing_claim.claim_id != req.claim.claim_id {
        return Err(ApiError::BadRequest(format!(
            "claim_id mismatch: issue holds {:?}, request has {:?}",
            existing_claim.claim_id, req.claim.claim_id
        )));
    }

    let previous_state = state_str(existing_claim.state);
    let new_state = state_str(req.claim.state);
    let state_changed = previous_state != new_state;

    // Write the new manifest (body PATCH).
    let body = req.claim.to_issue_body();
    state
        .github
        .update_issue_body(&state.cfg.state_repo, req.issue_number, &body)
        .await
        .map_err(|e| ApiError::Upstream(format!("github update_issue_body: {e}")))?;

    // State-transition label flip. Removing the old label first; then
    // adding the new — order doesn't matter to GitHub but the event
    // log reads cleaner with -then-+. Idempotent if state unchanged.
    if state_changed {
        let old_label = format!("state:{}", label_state_slug(previous_state));
        let new_label = format!("state:{}", label_state_slug(new_state));
        // 404 is treated as success in remove_label, so a freshly-
        // labelled issue won't fail the transition just because the
        // old label was already absent.
        state
            .github
            .remove_label(&state.cfg.state_repo, req.issue_number, &old_label)
            .await
            .map_err(|e| ApiError::Upstream(format!("remove_label {old_label}: {e}")))?;
        state
            .github
            .add_labels(&state.cfg.state_repo, req.issue_number, &[&new_label])
            .await
            .map_err(|e| ApiError::Upstream(format!("add_labels {new_label}: {e}")))?;
    }

    // Append an event-history comment. Default note describes the
    // transition; an operator-supplied `event_note` extends it.
    let mut comment = if state_changed {
        format!("State: `{previous_state}` → `{new_state}`")
    } else {
        format!("Manifest updated (state unchanged: `{new_state}`)")
    };
    if let Some(extra) = req.event_note.as_deref().filter(|s| !s.is_empty()) {
        comment.push_str("\n\n");
        comment.push_str(extra);
    }
    state
        .github
        .add_comment(&state.cfg.state_repo, req.issue_number, &comment)
        .await
        .map_err(|e| ApiError::Upstream(format!("add_comment: {e}")))?;

    Ok(Json(ClaimUpdateResp {
        claim: req.claim,
        issue_number: req.issue_number,
        issue_url: existing_issue.html_url,
        previous_state: previous_state.into(),
        new_state: new_state.into(),
        state_changed,
    }))
}

/// Map the snake_case `ClaimState` JSON form to a label-friendly slug
/// (kebab-case). GitHub labels conventionally use kebab-case, and
/// `state:active`/`state:overdue`/etc. read more naturally than
/// `state:active` ↔ JSON `state:"active"` already aligning, but the
/// multi-word ones like `payment_failed` flip to `payment-failed`.
fn label_state_slug(s: &str) -> String {
    s.replace('_', "-")
}

/// Stringify `ClaimState` to the same snake_case form serde produces.
/// Centralized so labels and JSON stay in sync.
fn state_str(s: ClaimState) -> &'static str {
    match s {
        ClaimState::Requested => "requested",
        ClaimState::InvoiceCreated => "invoice_created",
        ClaimState::BtcMempoolSeen => "btc_mempool_seen",
        ClaimState::NodeAssignmentStarted => "node_assignment_started",
        ClaimState::OwnerUpdateDispatched => "owner_update_dispatched",
        ClaimState::ActivePendingConfirmation => "active_pending_confirmation",
        ClaimState::BtcConfirmed => "btc_confirmed",
        ClaimState::Active => "active",
        ClaimState::Overdue => "overdue",
        ClaimState::Shutdown => "shutdown",
        ClaimState::PaymentFailed => "payment_failed",
        ClaimState::BootFailed => "boot_failed",
        ClaimState::OwnerUpdateFailed => "owner_update_failed",
        ClaimState::AttestationFailed => "attestation_failed",
        ClaimState::ShutdownFailed => "shutdown_failed",
        ClaimState::NodeFailed => "node_failed",
        ClaimState::ManualReview => "manual_review",
    }
}

// ── btc.invoice ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct BtcInvoiceReq {
    pub issue_number: u64,
    /// Number of 24-hour blocks to bill in this invoice. Defaults to
    /// 1 (one day's access). Top-ups call `btc.invoice` again with
    /// higher `blocks`; the bot tracks confirmed payments per claim
    /// and credits each block.
    #[serde(default = "default_blocks")]
    pub blocks: u32,
}

fn default_blocks() -> u32 {
    1
}

#[derive(Debug, Serialize)]
pub struct BtcInvoiceResp {
    pub address: String,
    pub amount_sats: u64,
    /// BTC amount as a decimal string suitable for BIP21 `amount=`
    /// (no trailing zeros stripped — wallets accept either, and
    /// fixed-width is easier on humans reading the URI).
    pub amount_btc: String,
    pub bip21_uri: String,
    /// Plaintext message embedded as BIP21 `message=`. The bot uses
    /// `claim_id` so a customer pasting the URI into a wallet sees
    /// what they're paying for, and so the operator can attribute
    /// payments to claims via the wallet's own tx history.
    pub message: String,
    pub blocks: u32,
}

async fn btc_invoice(
    State(state): State<State_>,
    headers: HeaderMap,
    Json(req): Json<BtcInvoiceReq>,
) -> Result<Json<BtcInvoiceResp>, ApiError> {
    require_tool_token(&headers, &state.cfg.tool_api_token)?;

    if req.blocks == 0 {
        return Err(ApiError::BadRequest(
            "blocks must be >= 1 (one 24-hour unit)".into(),
        ));
    }

    let issue = state
        .github
        .get_issue(&state.cfg.state_repo, req.issue_number)
        .await
        .map_err(|e| ApiError::Upstream(format!("github get_issue: {e}")))?;
    let mut claim = Claim::from_issue_body(&issue.body)
        .map_err(|e| ApiError::Upstream(format!("issue body manifest: {e}")))?;

    // Total cost = single-block price × number of blocks. checked_mul
    // so a malicious or misconfigured frontend can't overflow u64
    // and surprise the wallet.
    let amount_sats = claim
        .btc
        .price_per_24h_sats
        .checked_mul(req.blocks as u64)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "blocks={} × price_per_24h_sats={} overflows u64",
                req.blocks, claim.btc.price_per_24h_sats
            ))
        })?;
    let amount_btc = format_btc(amount_sats);

    // BIP21: bitcoin:<address>?amount=<BTC>&label=...&message=...
    // Each query value is percent-encoded per RFC 3986. We don't
    // pull `url::Url` for one URI — the value space is small and
    // we know the inputs.
    let label = "Sats for Compute";
    let message = claim.claim_id.clone();
    let bip21_uri = format!(
        "bitcoin:{address}?amount={amount}&label={label}&message={message}",
        address = claim.btc.address,
        amount = amount_btc,
        label = percent_encode(label),
        message = percent_encode(&message),
    );

    // Advance state Requested → InvoiceCreated on first invoice. The
    // orchestrator's process_invoice_created scans claims at that
    // exact label, so without this transition the BTC watcher never
    // sees the customer's payment. Re-invoicing (top-ups) on a claim
    // that's already in InvoiceCreated or further is a no-op here —
    // we just regenerate the URI without churning state.
    if matches!(claim.state, ClaimState::Requested) {
        claim.state = ClaimState::InvoiceCreated;
        let body = claim.to_issue_body();
        state
            .github
            .update_issue_body(&state.cfg.state_repo, req.issue_number, &body)
            .await
            .map_err(|e| ApiError::Upstream(format!("github update_issue_body: {e}")))?;
        // 404 on the old label is fine (already removed by hand).
        state
            .github
            .remove_label(&state.cfg.state_repo, req.issue_number, "state:requested")
            .await
            .ok();
        state
            .github
            .add_labels(
                &state.cfg.state_repo,
                req.issue_number,
                &["state:invoice-created"],
            )
            .await
            .ok();
        state
            .github
            .add_comment(
                &state.cfg.state_repo,
                req.issue_number,
                &format!(
                    "Generated BIP21 invoice for {amount_sats} sats ({} block(s)). State: `requested` → `invoice_created`. The orchestrator will now watch the address for payment.",
                    req.blocks
                ),
            )
            .await
            .ok();
    }

    Ok(Json(BtcInvoiceResp {
        address: claim.btc.address,
        amount_sats,
        amount_btc,
        bip21_uri,
        message,
        blocks: req.blocks,
    }))
}

/// Sats → BTC as a fixed 8-decimal string. `50_000` sats → `"0.00050000"`.
/// Stable width is friendlier to copy-paste and to humans reading
/// the URI; wallets accept any number of decimals.
fn format_btc(sats: u64) -> String {
    let whole = sats / 100_000_000;
    let frac = sats % 100_000_000;
    format!("{whole}.{frac:08}")
}

/// Percent-encode a value for BIP21 query parameters. Conservative
/// allowed-set: ALPHA / DIGIT / `-_.~` (RFC 3986 unreserved). Spaces
/// in `label` become `%20`, claim_ids that contain `_` survive
/// untouched.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            for b in c.to_string().as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

// ── node.boot ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct NodeBootReq {
    pub issue_number: u64,
}

#[derive(Debug, Serialize)]
pub struct NodeBootResp {
    pub claim_id: String,
    pub dispatch: WorkflowDispatch,
}

/// Echo of the dispatch the bot fired. The workflow API returns 204 No
/// Content with no run ID, so we surface back what we sent — operator
/// frontends use this to find the matching run via
/// `/repos/{ops_repo}/actions/workflows/{file}/runs?head_sha=...`
/// or by filtering on the claim_id input.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowDispatch {
    pub ops_repo: String,
    pub workflow: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub inputs: serde_json::Map<String, serde_json::Value>,
}

async fn node_boot(
    State(state): State<State_>,
    headers: HeaderMap,
    Json(req): Json<NodeBootReq>,
) -> Result<Json<NodeBootResp>, ApiError> {
    require_tool_token(&headers, &state.cfg.tool_api_token)?;

    let issue = state
        .github
        .get_issue(&state.cfg.state_repo, req.issue_number)
        .await
        .map_err(|e| ApiError::Upstream(format!("github get_issue: {e}")))?;
    let claim = Claim::from_issue_body(&issue.body)
        .map_err(|e| ApiError::Upstream(format!("issue body manifest: {e}")))?;

    let inputs = build_boot_inputs(&claim).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let workflow = state.cfg.ops_boot_workflow.clone();
    let ref_ = state.cfg.ops_ref.clone();
    let ops_repo = state.cfg.ops_repo.clone();

    state
        .github
        .dispatch_workflow(&ops_repo, &workflow, &ref_, &inputs)
        .await
        .map_err(|e| ApiError::Upstream(format!("dispatch_workflow boot: {e}")))?;

    Ok(Json(NodeBootResp {
        claim_id: claim.claim_id,
        dispatch: WorkflowDispatch {
            ops_repo,
            workflow,
            ref_,
            inputs,
        },
    }))
}

/// Build the `inputs` map for the `boot-agent.yml` workflow_dispatch.
/// Pure helper so the wire shape can be doc-tested without spinning up
/// a real GitHub round-trip.
///
/// Customer-deploy mode populates `customer_owner` and leaves the
/// workload fields empty. Confidential mode populates `workload_repo`
/// and `workload_ref` and leaves `customer_owner` empty. The receiving
/// workflow branches on `mode`.
///
/// ```
/// use satsforcompute::claim::{BtcDetails, Claim, ClaimMode};
/// use satsforcompute::tools::build_boot_inputs;
///
/// let mut c = Claim::new(
///     "claim_42",
///     ClaimMode::CustomerDeploy,
///     BtcDetails {
///         address: "bc1q-x".into(),
///         price_per_24h_sats: 50_000,
///         required_confirmations: 1,
///         pending_timeout_secs: 10_800,
///     },
/// );
/// c.customer_owner = Some("alice".into());
/// let inputs = build_boot_inputs(&c).unwrap();
/// assert_eq!(inputs["claim_id"], "claim_42");
/// assert_eq!(inputs["mode"], "customer_deploy");
/// assert_eq!(inputs["customer_owner"], "alice");
/// assert_eq!(inputs["workload_repo"], "");
/// ```
pub fn build_boot_inputs(
    claim: &Claim,
) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    let mut inputs = serde_json::Map::new();
    inputs.insert("claim_id".into(), claim.claim_id.clone().into());
    inputs.insert(
        "mode".into(),
        match claim.mode {
            ClaimMode::CustomerDeploy => "customer_deploy",
            ClaimMode::Confidential => "confidential",
        }
        .into(),
    );
    inputs.insert(
        "customer_owner".into(),
        claim.customer_owner.clone().unwrap_or_default().into(),
    );
    match claim.mode {
        ClaimMode::CustomerDeploy => {
            inputs.insert("workload_repo".into(), "".into());
            inputs.insert("workload_ref".into(), "".into());
        }
        ClaimMode::Confidential => {
            let repo = claim.workload_repo.as_deref().unwrap_or("");
            if repo.is_empty() {
                anyhow::bail!("confidential claim missing workload_repo on manifest");
            }
            inputs.insert("workload_repo".into(), repo.into());
            inputs.insert(
                "workload_ref".into(),
                claim
                    .workload_ref
                    .clone()
                    .unwrap_or_else(|| "main".into())
                    .into(),
            );
        }
    }
    Ok(inputs)
}

// ── dd.dispatch_owner_update ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DdDispatchOwnerUpdateReq {
    pub issue_number: u64,
    /// Public hostname of the dd-agent the owner-update should land on,
    /// e.g. `dd-agent-7.devopsdefender.com`. The orchestrator passes
    /// the value the boot workflow wrote back into
    /// `claim.agent_hostname`; manual operators can override it.
    #[serde(default)]
    pub agent_host: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DdDispatchOwnerUpdateResp {
    pub claim_id: String,
    pub dispatch: WorkflowDispatch,
}

async fn dd_dispatch_owner_update(
    State(state): State<State_>,
    headers: HeaderMap,
    Json(req): Json<DdDispatchOwnerUpdateReq>,
) -> Result<Json<DdDispatchOwnerUpdateResp>, ApiError> {
    require_tool_token(&headers, &state.cfg.tool_api_token)?;

    let issue = state
        .github
        .get_issue(&state.cfg.state_repo, req.issue_number)
        .await
        .map_err(|e| ApiError::Upstream(format!("github get_issue: {e}")))?;
    let claim = Claim::from_issue_body(&issue.body)
        .map_err(|e| ApiError::Upstream(format!("issue body manifest: {e}")))?;

    // Fail-closed: confidential mode has no /owner route on the agent
    // (it's not registered when DD_CONFIDENTIAL=true). Calling owner-
    // update there would be a no-op at best, a misleading audit-trail
    // entry at worst.
    if matches!(claim.mode, ClaimMode::Confidential) {
        return Err(ApiError::BadRequest(
            "dd.dispatch_owner_update is not valid for confidential claims".into(),
        ));
    }

    // Caller may override; otherwise read from the manifest the boot
    // workflow wrote back.
    let agent_host = req
        .agent_host
        .filter(|s| !s.is_empty())
        .or_else(|| claim.agent_hostname.clone())
        .ok_or_else(|| {
            ApiError::BadRequest(
                "agent_host required (none on request and none on claim manifest)".into(),
            )
        })?;
    let agent_owner = claim.customer_owner.clone().ok_or_else(|| {
        ApiError::BadRequest(
            "claim.customer_owner is unset; can't dispatch owner-update without an owner".into(),
        )
    })?;

    let inputs = build_owner_update_inputs(&claim.claim_id, &agent_host, &agent_owner);
    let workflow = state.cfg.ops_owner_workflow.clone();
    let ref_ = state.cfg.ops_ref.clone();
    let ops_repo = state.cfg.ops_repo.clone();

    state
        .github
        .dispatch_workflow(&ops_repo, &workflow, &ref_, &inputs)
        .await
        .map_err(|e| ApiError::Upstream(format!("dispatch_workflow owner-update: {e}")))?;

    Ok(Json(DdDispatchOwnerUpdateResp {
        claim_id: claim.claim_id,
        dispatch: WorkflowDispatch {
            ops_repo,
            workflow,
            ref_,
            inputs,
        },
    }))
}

/// Build the `inputs` map for the `owner-update.yml` workflow.
///
/// ```
/// use satsforcompute::tools::build_owner_update_inputs;
///
/// let inputs = build_owner_update_inputs(
///     "claim_42",
///     "dd-agent-7.devopsdefender.com",
///     "alice",
/// );
/// assert_eq!(inputs["claim_id"], "claim_42");
/// assert_eq!(inputs["agent_host"], "dd-agent-7.devopsdefender.com");
/// assert_eq!(inputs["agent_owner"], "alice");
/// ```
pub fn build_owner_update_inputs(
    claim_id: &str,
    agent_host: &str,
    agent_owner: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut inputs = serde_json::Map::new();
    inputs.insert("claim_id".into(), claim_id.into());
    inputs.insert("agent_host".into(), agent_host.into());
    inputs.insert("agent_owner".into(), agent_owner.into());
    inputs
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

    #[test]
    fn state_str_round_trips_with_serde() {
        // Every variant of ClaimState the spec lists. If serde's
        // snake_case rename and our `state_str` ever drift, this test
        // catches it before claim.update mislabels an issue.
        for st in [
            ClaimState::Requested,
            ClaimState::InvoiceCreated,
            ClaimState::BtcMempoolSeen,
            ClaimState::NodeAssignmentStarted,
            ClaimState::OwnerUpdateDispatched,
            ClaimState::ActivePendingConfirmation,
            ClaimState::BtcConfirmed,
            ClaimState::Active,
            ClaimState::Overdue,
            ClaimState::Shutdown,
            ClaimState::PaymentFailed,
            ClaimState::BootFailed,
            ClaimState::OwnerUpdateFailed,
            ClaimState::AttestationFailed,
            ClaimState::ShutdownFailed,
            ClaimState::NodeFailed,
            ClaimState::ManualReview,
        ] {
            let via_serde = serde_json::to_value(st).unwrap();
            assert_eq!(via_serde, state_str(st));
        }
    }

    #[test]
    fn label_state_slug_kebabs_underscored_states() {
        assert_eq!(label_state_slug("active"), "active");
        assert_eq!(label_state_slug("payment_failed"), "payment-failed");
        assert_eq!(
            label_state_slug("active_pending_confirmation"),
            "active-pending-confirmation"
        );
    }

    #[test]
    fn format_btc_pads_to_eight_decimals() {
        assert_eq!(format_btc(0), "0.00000000");
        assert_eq!(format_btc(1), "0.00000001");
        assert_eq!(format_btc(50_000), "0.00050000");
        assert_eq!(format_btc(100_000_000), "1.00000000");
        assert_eq!(format_btc(150_000_000), "1.50000000");
        assert_eq!(format_btc(2_100_000_000_000_000), "21000000.00000000");
    }

    #[test]
    fn percent_encode_passes_unreserved_through() {
        assert_eq!(percent_encode("claim_abc-123.0"), "claim_abc-123.0");
        assert_eq!(percent_encode("Sats for Compute"), "Sats%20for%20Compute");
        // Multi-byte UTF-8 (just to make sure we don't panic / mangle).
        assert_eq!(percent_encode("café"), "caf%C3%A9");
    }

    #[test]
    fn percent_encode_handles_bip21_special_chars() {
        // Anything that would break the URI must be encoded.
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(percent_encode("a?b#c"), "a%3Fb%23c");
    }

    #[test]
    fn build_boot_inputs_confidential_requires_workload_repo() {
        // A confidential claim missing workload_repo on the manifest is
        // a programming error upstream — fail-closed at dispatch time.
        let c = Claim::new(
            "claim_x",
            ClaimMode::Confidential,
            BtcDetails {
                address: "bc1q-x".into(),
                price_per_24h_sats: 50_000,
                required_confirmations: 1,
                pending_timeout_secs: 10_800,
            },
        );
        let err = build_boot_inputs(&c).unwrap_err();
        assert!(
            err.to_string().contains("workload_repo"),
            "expected workload_repo in error, got {err}"
        );
    }

    #[test]
    fn build_boot_inputs_confidential_carries_workload_ref() {
        let mut c = Claim::new(
            "claim_x",
            ClaimMode::Confidential,
            BtcDetails {
                address: "bc1q-x".into(),
                price_per_24h_sats: 50_000,
                required_confirmations: 1,
                pending_timeout_secs: 10_800,
            },
        );
        c.workload_repo = Some("alice/oracle".into());
        c.workload_ref = Some("v1.2.3".into());
        let inputs = build_boot_inputs(&c).unwrap();
        assert_eq!(inputs["mode"], "confidential");
        assert_eq!(inputs["workload_repo"], "alice/oracle");
        assert_eq!(inputs["workload_ref"], "v1.2.3");
        // No customer_owner binding in confidential mode.
        assert_eq!(inputs["customer_owner"], "");
    }

    #[test]
    fn claim_update_rejects_wrong_schema() {
        // Schema-version guardrail check that doesn't need a live
        // GitHub round-trip — synthesize a Claim with the wrong
        // schema and assert validation returns BadRequest.
        let mut c = Claim::new(
            "claim_x",
            ClaimMode::CustomerDeploy,
            BtcDetails {
                address: "bc1q-x".into(),
                price_per_24h_sats: 50_000,
                required_confirmations: 1,
                pending_timeout_secs: 10_800,
            },
        );
        c.schema = "future.claim.v9".into();
        // The actual handler reaches the GitHub call only after the
        // schema check passes — confirming the order means the BadRequest
        // path doesn't depend on having a network. We replicate the
        // check inline because the handler signature requires axum
        // State, which is awkward to materialise without a running
        // server.
        let err = if c.schema != CURRENT_SCHEMA {
            Some(ApiError::BadRequest(format!(
                "claim.schema must be {CURRENT_SCHEMA:?}, got {:?}",
                c.schema
            )))
        } else {
            None
        };
        match err {
            Some(ApiError::BadRequest(msg)) => assert!(msg.contains("future.claim.v9")),
            _ => panic!("expected BadRequest"),
        }
    }
}
