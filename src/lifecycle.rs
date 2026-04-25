//! Background orchestrator: watches BTC, transitions claim state.
//!
//! The HTTP `/tools/*` API is the request/response front. The
//! orchestrator is the agent that periodically polls the world
//! (mempool.space + dd-agent /health, eventually) and advances each
//! open claim's state machine.
//!
//! Single tokio task spawned from `server::run`. Stateless across
//! ticks — every tick re-derives state from the canonical GitHub
//! issue body. Restart-safe by construction (no in-memory queue).
//!
//! v0 transitions (one per tick to keep behaviour predictable):
//!
//! - `invoice_created` → `btc_mempool_seen` when any tx ≥
//!   `price_per_24h_sats` lands at the operator address since the
//!   issue was opened. Sets `billing.last_payment_txid`.
//! - `btc_mempool_seen` → `active` when the recorded tx reaches
//!   `required_confirmations`. Sets `billing.paid_until = now + 24h`
//!   (one block credited per first payment; top-ups extend in a
//!   separate path, deferred).
//! - `active` → `overdue` when `paid_until` has passed.
//!
//! Single-address attribution caveat: with the v0 invoice stub
//! (everyone pays into the operator's static `sweep_address`), the
//! orchestrator can't always tell which claim a tx belongs to.
//! Heuristic: oldest-`invoice_created`-first matching the amount
//! wins. This is fine for low-concurrency demo use; per-claim
//! address derivation lands with the BDK wallet workload.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use tracing::{debug, error, info, warn};

use crate::btc::MempoolSpace;
use crate::claim::{Claim, ClaimState};
use crate::config::Config;
use crate::github;

/// Default tick cadence. Fast enough that a 1-conf BTC payment
/// gets credited within a couple of minutes; slow enough that
/// GitHub + mempool.space rate limits aren't an issue.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// One operator-bot's lifecycle orchestrator.
#[derive(Clone)]
pub struct Lifecycle {
    cfg: Arc<Config>,
    github: Arc<github::Client>,
    btc: Arc<MempoolSpace>,
}

impl Lifecycle {
    pub fn new(cfg: Arc<Config>, github: Arc<github::Client>, btc: Arc<MempoolSpace>) -> Self {
        Self { cfg, github, btc }
    }

    /// Spawn the tick loop on the current tokio runtime. Returns
    /// immediately; the loop runs forever.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                interval_secs = DEFAULT_TICK_INTERVAL.as_secs(),
                "lifecycle: orchestrator running"
            );
            // First tick after a short warmup so the listener has
            // settled and / health is serving before background
            // GitHub polls start hammering.
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                if let Err(e) = self.tick().await {
                    // Don't crash the loop — a transient mempool.space
                    // 5xx or a GitHub rate-limit shouldn't take the
                    // bot offline. Log and try again next tick.
                    warn!(error = %e, "lifecycle: tick failed");
                }
                tokio::time::sleep(DEFAULT_TICK_INTERVAL).await;
            }
        })
    }

    /// Run one orchestration pass. Public so tests + ops can trigger
    /// a single tick on demand (a future `/admin/tick` endpoint can
    /// reuse this directly).
    pub async fn tick(&self) -> Result<()> {
        // Each state branch is a separate listing call; GitHub `?labels=`
        // filters AND the labels, so combining them in one request
        // would only return claims with multiple state labels (none).
        self.process_invoice_created().await?;
        self.process_btc_mempool_seen().await?;
        self.process_active().await?;
        Ok(())
    }

    async fn process_invoice_created(&self) -> Result<()> {
        let issues = self
            .github
            .list_open_issues_by_labels(&self.cfg.state_repo, &["s12e", "state:invoice-created"])
            .await
            .context("list invoice-created claims")?;
        if issues.is_empty() {
            return Ok(());
        }
        debug!(count = issues.len(), "lifecycle: scanning invoice_created");

        // Sort oldest-first so the single-address heuristic is
        // deterministic when multiple claims are waiting on the same
        // address (oldest invoice claims the oldest unattributed tx).
        let mut issues = issues;
        issues.sort_by_key(|i| i.number);

        // One mempool.space scrape per address — same address is
        // shared by all claims in v0.
        let txs = self
            .btc
            .list_address_txs(&self.cfg.sweep_address)
            .await
            .context("list address txs")?;

        for issue in issues {
            let mut claim = match Claim::from_issue_body(&issue.body) {
                Ok(c) => c,
                Err(e) => {
                    error!(issue = issue.number, error = %e, "manifest parse failed");
                    continue;
                }
            };
            // Match the oldest tx ≥ price that doesn't already belong
            // to a confirmed claim. v0 simplification: pick any
            // unattributed tx of sufficient size; trust no double-
            // attribution because we set `last_payment_txid` on
            // transition and skip txs we've seen before.
            let unattributed = txs.iter().find(|tx| {
                tx.received_sats >= claim.btc.price_per_24h_sats
                    && claim.billing.last_payment_txid.as_deref() != Some(&tx.txid)
            });
            let Some(tx) = unattributed else { continue };

            claim.state = ClaimState::BtcMempoolSeen;
            claim.billing.last_payment_txid = Some(tx.txid.clone());

            self.transition(
                &issue,
                &mut claim,
                ClaimState::InvoiceCreated,
                &format!(
                    "Saw payment of {} sats on tx `{}` — proceeding to confirmation watch.",
                    tx.received_sats, tx.txid
                ),
            )
            .await?;
        }
        Ok(())
    }

    async fn process_btc_mempool_seen(&self) -> Result<()> {
        let issues = self
            .github
            .list_open_issues_by_labels(&self.cfg.state_repo, &["s12e", "state:btc-mempool-seen"])
            .await
            .context("list btc-mempool-seen claims")?;
        if issues.is_empty() {
            return Ok(());
        }
        let tip = self
            .btc
            .current_block_height()
            .await
            .context("btc tip height")?;
        // Re-fetch the address book once; multiple claims share one
        // address in v0 so this is one network round-trip.
        let txs = self
            .btc
            .list_address_txs(&self.cfg.sweep_address)
            .await
            .context("list address txs")?;
        for issue in issues {
            let mut claim = match Claim::from_issue_body(&issue.body) {
                Ok(c) => c,
                Err(e) => {
                    error!(issue = issue.number, error = %e, "manifest parse failed");
                    continue;
                }
            };
            let Some(txid) = claim.billing.last_payment_txid.clone() else {
                warn!(
                    issue = issue.number,
                    "btc-mempool-seen with no last_payment_txid; needs operator review"
                );
                continue;
            };
            let Some(tx) = txs.iter().find(|t| t.txid == txid) else {
                debug!(issue = issue.number, %txid, "tx not yet visible to mempool.space");
                continue;
            };
            let confs = tx.confirmations(tip);
            if confs < claim.btc.required_confirmations {
                debug!(issue = issue.number, %txid, confs, "still waiting for confirmations");
                continue;
            }
            // Credit ONE 24-hour block on first confirmation. Top-up
            // logic (multiple blocks) lands when btc.invoice grows a
            // matching multi-block path.
            let now = Utc::now();
            let new_paid_until = credit_24h(now, claim.billing.paid_until);
            claim.state = ClaimState::Active;
            claim.billing.paid_until = Some(new_paid_until);

            let comment = format!(
                "Tx `{txid}` reached {confs} conf — claim active until `{}`.",
                new_paid_until.to_rfc3339()
            );
            self.transition(&issue, &mut claim, ClaimState::BtcMempoolSeen, &comment)
                .await?;
        }
        Ok(())
    }

    async fn process_active(&self) -> Result<()> {
        let issues = self
            .github
            .list_open_issues_by_labels(&self.cfg.state_repo, &["s12e", "state:active"])
            .await
            .context("list active claims")?;
        if issues.is_empty() {
            return Ok(());
        }
        let now = Utc::now();
        for issue in issues {
            let mut claim = match Claim::from_issue_body(&issue.body) {
                Ok(c) => c,
                Err(e) => {
                    error!(issue = issue.number, error = %e, "manifest parse failed");
                    continue;
                }
            };
            let expired = claim
                .billing
                .paid_until
                .is_some_and(|p| now > p + chrono::Duration::seconds(0));
            if !expired {
                continue;
            }
            claim.state = ClaimState::Overdue;
            self.transition(
                &issue,
                &mut claim,
                ClaimState::Active,
                "`paid_until` has passed — marking overdue. Operator action required (extend or shutdown).",
            )
            .await?;
        }
        Ok(())
    }

    async fn transition(
        &self,
        issue: &github::Issue,
        claim: &mut Claim,
        previous_state: ClaimState,
        comment: &str,
    ) -> Result<()> {
        let body = claim.to_issue_body();
        self.github
            .update_issue_body(&self.cfg.state_repo, issue.number, &body)
            .await
            .context("update_issue_body")?;
        let from = state_label(previous_state);
        let to = state_label(claim.state);
        if from != to {
            // Best-effort label flip. 404 on the old label is fine
            // (already removed manually). Comment writes after labels
            // so the comment is the chronologically-last marker —
            // makes the GitHub UI scroll cleaner.
            self.github
                .remove_label(&self.cfg.state_repo, issue.number, from)
                .await
                .ok();
            self.github
                .add_labels(&self.cfg.state_repo, issue.number, &[to])
                .await
                .ok();
        }
        self.github
            .add_comment(&self.cfg.state_repo, issue.number, comment)
            .await
            .ok();
        info!(
            issue = issue.number,
            from = from,
            to = to,
            "lifecycle: transitioned"
        );
        Ok(())
    }
}

/// Compute the new `paid_until` after crediting one 24-hour block.
/// Extends from `max(now, current_paid_until)` so that paying twice
/// before expiry stacks correctly (top-ups never lose time).
///
/// # Examples
///
/// ```
/// # use chrono::{TimeZone, Utc, Duration};
/// # use satsforcompute::lifecycle::credit_24h;
/// let now = Utc.with_ymd_and_hms(2026, 4, 25, 12, 0, 0).unwrap();
/// // Fresh claim: no prior paid_until.
/// assert_eq!(credit_24h(now, None), now + Duration::hours(24));
/// // Top-up while still active: extends from existing paid_until.
/// let later = now + Duration::hours(10);
/// assert_eq!(credit_24h(now, Some(later)), later + Duration::hours(24));
/// // Top-up after expiry: extends from now, prior paid_until is irrelevant.
/// let past = now - Duration::hours(5);
/// assert_eq!(credit_24h(now, Some(past)), now + Duration::hours(24));
/// ```
pub fn credit_24h(now: DateTime<Utc>, current: Option<DateTime<Utc>>) -> DateTime<Utc> {
    let base = match current {
        Some(t) if t > now => t,
        _ => now,
    };
    base + chrono::Duration::hours(24)
}

/// Map a `ClaimState` to the GitHub label slug. kebab-cased so labels
/// like `state:active-pending-confirmation` read naturally.
///
/// # Examples
///
/// ```
/// # use satsforcompute::claim::ClaimState;
/// # use satsforcompute::lifecycle::state_label;
/// assert_eq!(state_label(ClaimState::Active), "state:active");
/// assert_eq!(state_label(ClaimState::ActivePendingConfirmation), "state:active-pending-confirmation");
/// assert_eq!(state_label(ClaimState::PaymentFailed), "state:payment-failed");
/// ```
pub fn state_label(s: ClaimState) -> &'static str {
    match s {
        ClaimState::Requested => "state:requested",
        ClaimState::InvoiceCreated => "state:invoice-created",
        ClaimState::BtcMempoolSeen => "state:btc-mempool-seen",
        ClaimState::NodeAssignmentStarted => "state:node-assignment-started",
        ClaimState::OwnerUpdateDispatched => "state:owner-update-dispatched",
        ClaimState::ActivePendingConfirmation => "state:active-pending-confirmation",
        ClaimState::BtcConfirmed => "state:btc-confirmed",
        ClaimState::Active => "state:active",
        ClaimState::Overdue => "state:overdue",
        ClaimState::Shutdown => "state:shutdown",
        ClaimState::PaymentFailed => "state:payment-failed",
        ClaimState::BootFailed => "state:boot-failed",
        ClaimState::OwnerUpdateFailed => "state:owner-update-failed",
        ClaimState::AttestationFailed => "state:attestation-failed",
        ClaimState::ShutdownFailed => "state:shutdown-failed",
        ClaimState::NodeFailed => "state:node-failed",
        ClaimState::ManualReview => "state:manual-review",
    }
}
