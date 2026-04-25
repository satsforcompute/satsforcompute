//! Canonical claim manifest schema (`s12e.claim.v1`).
//!
//! One claim corresponds to one customer's purchase of compute. The
//! manifest is the single canonical state document for the claim;
//! it lives as a JSON code-fence inside a GitHub issue body. Comments
//! on the same issue are append-only event history (not parsed).
//!
//! Keep this type stable across compatible additions; bump the schema
//! string when an existing field's meaning changes.
//!
//! Spec: SATS_FOR_COMPUTE_SPEC.md, "GitHub-Backed State" section.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA: &str = "s12e.claim.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    /// Schema discriminator. Required so future readers can fail-loud
    /// on a manifest written by a newer version they don't grok.
    pub schema: String,
    pub claim_id: String,
    pub state: ClaimState,
    /// GitHub user/org the customer wants `/deploy` etc. authority on.
    /// In confidential mode this stays None — the bot deploys a sealed
    /// workload, no tenant identity is bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_owner: Option<String>,
    /// Assigned dd-agent (set after node provisioning + /owner call).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Public hostname of the assigned agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_hostname: Option<String>,
    /// Mode the bot deployed in. Affects how /deploy and /owner work
    /// on the assigned agent — see SATS_FOR_COMPUTE_SPEC.md.
    pub mode: ClaimMode,
    /// Public GitHub repo (`owner/repo`) holding the customer's
    /// `workload.json` for confidential mode. The bot fetches it at
    /// `workload_ref` when the claim activates and the boot workflow
    /// dispatches. Unset for `customer_deploy` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_repo: Option<String>,
    /// Git ref inside `workload_repo` to fetch — branch, tag, or commit
    /// SHA. Defaults to `main` when set; pinned SHAs let a customer
    /// prove which exact code their TDX quote attests to. Unset for
    /// `customer_deploy` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_ref: Option<String>,
    pub btc: BtcDetails,
    pub billing: Billing,
    pub integrity: Integrity,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    Requested,
    InvoiceCreated,
    BtcMempoolSeen,
    NodeAssignmentStarted,
    OwnerUpdateDispatched,
    ActivePendingConfirmation,
    BtcConfirmed,
    Active,
    Overdue,
    Shutdown,
    PaymentFailed,
    BootFailed,
    OwnerUpdateFailed,
    AttestationFailed,
    ShutdownFailed,
    NodeFailed,
    ManualReview,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimMode {
    /// Customer gets `DD_AGENT_OWNER` — full /deploy, /exec, /logs,
    /// ttyd. Default product variant.
    CustomerDeploy,
    /// Bot deploys a sealed workload on an agent booted with
    /// `DD_CONFIDENTIAL=true`. No /deploy, /exec, /owner. Workload
    /// from a customer-specified public GitHub repo (`workload.json`
    /// at root). See SATS_FOR_COMPUTE_SPEC.md "Confidential mode".
    Confidential,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtcDetails {
    pub address: String,
    pub price_per_24h_sats: u64,
    pub required_confirmations: u32,
    pub pending_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Billing {
    /// When access lapses unless extended by another payment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid_until: Option<DateTime<Utc>>,
    /// Sats received but not yet credited to a 24h block (e.g. a
    /// partial payment under price_per_24h_sats).
    #[serde(default)]
    pub uncredited_sats: u64,
    /// Most recent BTC tx the lifecycle orchestrator attributed to
    /// this claim. Recorded so subsequent ticks know which tx to
    /// poll for confirmations + so the audit trail (issue comments)
    /// can link back to the on-chain proof. Cleared on top-up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_payment_txid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Integrity {
    /// Mirror of dd-agent's /health.confidential_mode. Cached here so
    /// claim consumers don't need a live agent fetch.
    #[serde(default)]
    pub confidential_mode: bool,
    /// Mirror of dd-agent's /health.taint_reasons.
    #[serde(default)]
    pub taint_reasons: Vec<TaintReason>,
}

/// Mirrors `devopsdefender::taint::TaintReason` so we can deserialize
/// the agent's /health response into our `Claim.integrity.taint_reasons`.
/// Defined locally (not re-exported from dd) to keep this crate
/// independently buildable.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Ord, PartialOrd, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TaintReason {
    CustomerOwnerEnabled,
    CustomerWorkloadDeployed,
    ArbitraryExecEnabled,
    InteractiveShellEnabled,
}

/// Markers around the canonical-JSON code-fence inside a claim issue
/// body. Anything else in the body is human-facing summary text and
/// is regenerated by `render_body`. The fence + the JSON between
/// these tags is the load-bearing part — issue comments, labels, and
/// human edits to the summary are all advisory.
const MANIFEST_OPEN: &str = "<!-- s12e:claim:v1:begin -->";
const MANIFEST_CLOSE: &str = "<!-- s12e:claim:v1:end -->";

impl Claim {
    /// New claim in the initial `requested` state, before BTC details
    /// are filled in. Caller bumps state as the lifecycle progresses.
    pub fn new(claim_id: impl Into<String>, mode: ClaimMode, btc: BtcDetails) -> Self {
        Self {
            schema: CURRENT_SCHEMA.into(),
            claim_id: claim_id.into(),
            state: ClaimState::Requested,
            customer_owner: None,
            agent_id: None,
            agent_hostname: None,
            mode,
            workload_repo: None,
            workload_ref: None,
            btc,
            billing: Billing {
                paid_until: None,
                uncredited_sats: 0,
                last_payment_txid: None,
            },
            integrity: Integrity {
                confidential_mode: matches!(mode, ClaimMode::Confidential),
                taint_reasons: Vec::new(),
            },
        }
    }

    /// Render the claim into a GitHub issue body. Format:
    ///
    /// 1. A short human-facing summary (state, customer, agent, paid_until).
    /// 2. An HTML-comment-bracketed code fence containing the canonical
    ///    JSON manifest. Bracket comments make the manifest invisible
    ///    in rendered Markdown but unambiguous to the parser.
    ///
    /// The renderer always rewrites the whole body — humans editing
    /// the summary are overwriting their own edits the next time the
    /// bot writes. That's intentional: the manifest is the source of
    /// truth, the summary is presentation.
    pub fn to_issue_body(&self) -> String {
        let customer = self
            .customer_owner
            .as_deref()
            .map(|s| format!("`{s}`"))
            .unwrap_or_else(|| "_(unset)_".into());
        let agent = self
            .agent_id
            .as_deref()
            .map(|s| format!("`{s}`"))
            .unwrap_or_else(|| "_(unassigned)_".into());
        let paid_until = self
            .billing
            .paid_until
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "_(not yet active)_".into());

        // Pretty-print the JSON so a human reading the issue body raw
        // can scan it. Round-trips identically through `from_issue_body`
        // via `serde_json::from_str` regardless of whitespace.
        let manifest = serde_json::to_string_pretty(self)
            .expect("Claim serializes (all fields are infallible)");

        format!(
            "# Sats for Compute claim `{claim_id}`\n\
             \n\
             - **State:** `{state}`\n\
             - **Mode:** `{mode}`\n\
             - **Customer owner:** {customer}\n\
             - **Agent:** {agent}\n\
             - **Paid until:** {paid_until}\n\
             \n\
             {open}\n\
             ```json\n\
             {manifest}\n\
             ```\n\
             {close}\n",
            claim_id = self.claim_id,
            state = state_str(self.state),
            mode = mode_str(self.mode),
            open = MANIFEST_OPEN,
            close = MANIFEST_CLOSE,
        )
    }

    /// Parse a claim out of a GitHub issue body. Locates the fenced
    /// JSON between `MANIFEST_OPEN` and `MANIFEST_CLOSE`, strips the
    /// ```json … ``` fence, and deserializes. Returns
    /// `ManifestError` on missing markers or unparseable JSON — both
    /// surface to the caller so a corrupted issue can be flagged
    /// rather than silently treated as a fresh claim.
    pub fn from_issue_body(body: &str) -> Result<Self, ManifestError> {
        let after_open =
            body.find(MANIFEST_OPEN).ok_or(ManifestError::MissingOpen)? + MANIFEST_OPEN.len();
        let close_idx = body[after_open..]
            .find(MANIFEST_CLOSE)
            .ok_or(ManifestError::MissingClose)?
            + after_open;
        let inner = &body[after_open..close_idx];

        // Strip the leading ```json and trailing ``` fences. Missing
        // either is a clear corruption — report rather than guess.
        let trimmed = inner.trim();
        let after_open_fence = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .ok_or(ManifestError::MissingFence)?;
        let json = after_open_fence
            .trim_end()
            .strip_suffix("```")
            .ok_or(ManifestError::MissingFence)?
            .trim();

        serde_json::from_str(json).map_err(ManifestError::Parse)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("issue body has no `{MANIFEST_OPEN}` marker")]
    MissingOpen,
    #[error("issue body has no `{MANIFEST_CLOSE}` marker")]
    MissingClose,
    #[error("manifest fence (` ```json ... ``` `) malformed")]
    MissingFence,
    #[error("manifest JSON: {0}")]
    Parse(#[from] serde_json::Error),
}

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

fn mode_str(m: ClaimMode) -> &'static str {
    match m {
        ClaimMode::CustomerDeploy => "customer_deploy",
        ClaimMode::Confidential => "confidential",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_constant_round_trips_through_json() {
        let c = Claim::new(
            "claim_test",
            ClaimMode::CustomerDeploy,
            BtcDetails {
                address: "bc1qtest".into(),
                price_per_24h_sats: 50_000,
                required_confirmations: 1,
                pending_timeout_secs: 10_800,
            },
        );
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["schema"], CURRENT_SCHEMA);
        assert_eq!(json["state"], "requested");
        assert_eq!(json["mode"], "customer_deploy");
        assert_eq!(json["integrity"]["confidential_mode"], false);
    }

    #[test]
    fn confidential_mode_sets_integrity_default() {
        let c = Claim::new(
            "claim_x",
            ClaimMode::Confidential,
            BtcDetails {
                address: "bc1qx".into(),
                price_per_24h_sats: 50_000,
                required_confirmations: 1,
                pending_timeout_secs: 10_800,
            },
        );
        assert!(c.integrity.confidential_mode);
        assert!(c.integrity.taint_reasons.is_empty());
    }

    #[test]
    fn taint_reason_serializes_snake_case() {
        let s = serde_json::to_string(&TaintReason::CustomerOwnerEnabled).unwrap();
        assert_eq!(s, "\"customer_owner_enabled\"");
    }

    #[test]
    fn body_round_trips_through_render_and_parse() {
        let mut c = Claim::new(
            "claim_round_trip",
            ClaimMode::CustomerDeploy,
            BtcDetails {
                address: "bc1qroundtrip".into(),
                price_per_24h_sats: 50_000,
                required_confirmations: 1,
                pending_timeout_secs: 10_800,
            },
        );
        c.customer_owner = Some("alice".into());
        c.agent_id = Some("dd-agent-7".into());
        c.agent_hostname = Some("https://dd-agent-7.devopsdefender.com".into());
        c.state = ClaimState::Active;
        c.integrity.taint_reasons = vec![
            TaintReason::CustomerWorkloadDeployed,
            TaintReason::CustomerOwnerEnabled,
        ];

        let body = c.to_issue_body();
        // Sanity: humans see the summary above the manifest fence.
        assert!(body.contains("# Sats for Compute claim"));
        assert!(body.contains("`alice`"));
        assert!(body.contains("```json"));

        let parsed = Claim::from_issue_body(&body).expect("parse");
        let original_json = serde_json::to_value(&c).unwrap();
        let parsed_json = serde_json::to_value(&parsed).unwrap();
        assert_eq!(original_json, parsed_json);
    }

    #[test]
    fn parse_tolerates_human_summary_edits() {
        // An operator might edit the human-facing markdown above the
        // fence; the manifest below should still parse.
        let mut c = Claim::new(
            "claim_tolerant",
            ClaimMode::Confidential,
            BtcDetails {
                address: "bc1qsealed".into(),
                price_per_24h_sats: 50_000,
                required_confirmations: 1,
                pending_timeout_secs: 10_800,
            },
        );
        c.state = ClaimState::Active;

        let body = c.to_issue_body();
        // Simulate a human appending a note at the top.
        let edited = format!("> note from ops: investigating slow start\n\n{body}");
        let parsed = Claim::from_issue_body(&edited).expect("parse");
        assert_eq!(parsed.claim_id, "claim_tolerant");
        assert_eq!(parsed.mode, ClaimMode::Confidential);
    }

    #[test]
    fn parse_errors_distinguish_failure_modes() {
        // No open marker.
        match Claim::from_issue_body("just some text").unwrap_err() {
            ManifestError::MissingOpen => {}
            other => panic!("expected MissingOpen, got {other:?}"),
        }
        // Open but no close.
        let s = format!("{MANIFEST_OPEN}\n```json\n{{}}\n```");
        match Claim::from_issue_body(&s).unwrap_err() {
            ManifestError::MissingClose => {}
            other => panic!("expected MissingClose, got {other:?}"),
        }
        // No fence around the JSON.
        let s = format!("{MANIFEST_OPEN}\nplain text\n{MANIFEST_CLOSE}");
        match Claim::from_issue_body(&s).unwrap_err() {
            ManifestError::MissingFence => {}
            other => panic!("expected MissingFence, got {other:?}"),
        }
        // Garbage inside the fence.
        let s = format!("{MANIFEST_OPEN}\n```json\nnot-json\n```\n{MANIFEST_CLOSE}");
        match Claim::from_issue_body(&s).unwrap_err() {
            ManifestError::Parse(_) => {}
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn claim_state_transitions_round_trip() {
        for state in [
            ClaimState::Requested,
            ClaimState::Active,
            ClaimState::Overdue,
            ClaimState::ManualReview,
            ClaimState::NodeFailed,
        ] {
            let s = serde_json::to_string(&state).unwrap();
            let back: ClaimState = serde_json::from_str(&s).unwrap();
            assert_eq!(back, state);
        }
    }
}
