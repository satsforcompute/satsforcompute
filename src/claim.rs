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
            btc,
            billing: Billing {
                paid_until: None,
                uncredited_sats: 0,
            },
            integrity: Integrity {
                confidential_mode: matches!(mode, ClaimMode::Confidential),
                taint_reasons: Vec::new(),
            },
        }
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
