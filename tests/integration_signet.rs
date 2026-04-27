//! End-to-end happy-path test against real signet + a real dd-agent.
//!
//! Self-driving: a BDK signet wallet broadcasts the customer's
//! payment of exactly `BtcDetails.exact_amount_sats` to the bot's
//! invoice address (a fresh receive address from the same wallet,
//! handed to the bot as `SATS_SWEEP_ADDRESS`). No fakes, no mocks,
//! no manual faucet drip between runs once the seed is funded once.
//!
//! Marked `#[ignore]` and gated:
//!
//! ```
//! SIGNET_SMOKE=1 cargo test --test integration_signet -- --ignored --nocapture
//! ```
//!
//! Required env:
//!
//! - `SATS_TEST_GH_PAT` — PAT for `state_repo` + `ops_repo` (repo:issues
//!   on state, actions:write on ops).
//! - `SATS_TEST_STATE_REPO` — `owner/repo` for claim issues.
//! - `SATS_TEST_OPS_REPO` — `owner/repo` for `repository_dispatch`.
//! - `SATS_TEST_CUSTOMER_OWNER` — GitHub login the claim grants.
//! - `SATS_TEST_DD_AGENT_HOST` — hostname of the test dd-agent (no
//!   scheme, e.g. `dd-local-bot.example.com`). The bot's tick handler
//!   polls `https://${host}/health` and the owner-update workflow
//!   POSTs to `https://${host}/owner`.
//! - `SATS_TEST_DD_AGENT_ID` — dd-agent identifier on that VM (used
//!   in claim.update so the audit trail names the right agent).
//! - `SATS_TEST_SIGNET_DESCRIPTOR` + `SATS_TEST_SIGNET_CHANGE_DESCRIPTOR`
//!   — see `tests/common/signet_wallet.rs` for the funding flow.
//! - `SATS_TEST_DD_AUTH_TOKEN` — optional Cloudflare Access bearer
//!   token for `/health` polls.
//!
//! Optimistic 0-conf path means we don't wait for a signet
//! confirmation (those are ~10 min, signet is also non-deterministic).
//! End-to-end takes ~1–3 min depending on workflow latency.

#[path = "common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bdk_wallet::KeychainKind;
use serde_json::{Value, json};

use crate::common::{BotHarness, signet_wallet::SignetWallet};

/// Per-state polling cap. Generous enough for workflow latency
/// (the slowest leg) but tight enough that a wedged step fails the
/// test before a CI runner times out the whole job.
const STATE_TIMEOUT: Duration = Duration::from_secs(300);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "signet smoke; gate via SIGNET_SMOKE=1 and signet env vars"]
async fn full_happy_path_signet() -> Result<()> {
    if std::env::var("SIGNET_SMOKE").ok().as_deref() != Some("1") {
        eprintln!("integ: skipping — set SIGNET_SMOKE=1 to run");
        return Ok(());
    }

    let gh_pat = require_env_or_skip!("SATS_TEST_GH_PAT");
    let state_repo = require_env_or_skip!("SATS_TEST_STATE_REPO");
    let ops_repo = require_env_or_skip!("SATS_TEST_OPS_REPO");
    let customer = require_env_or_skip!("SATS_TEST_CUSTOMER_OWNER");
    let dd_agent_host = require_env_or_skip!("SATS_TEST_DD_AGENT_HOST");
    let dd_agent_id = require_env_or_skip!("SATS_TEST_DD_AGENT_ID");
    let dd_auth_token = std::env::var("SATS_TEST_DD_AUTH_TOKEN").ok();

    let mempool_base = std::env::var("SATS_TEST_SIGNET_MEMPOOL_BASE")
        .unwrap_or_else(|_| "https://mempool.space/signet/api".into());

    // BDK wallet: must come up + fund-check before we touch the bot.
    let mut wallet = match SignetWallet::from_env_or_skip().await? {
        Some(w) => w,
        None => return Ok(()),
    };
    // Per-test cost ceiling: price (1000) + LSD (≤9999) + fee buffer.
    wallet.ensure_funded(20_000)?;
    let bot_sweep_addr = wallet
        .next_unused_address(KeychainKind::External)
        .to_string();
    eprintln!("integ: bot sweep address (fresh from wallet): {bot_sweep_addr}");

    let bot = BotHarness::spawn(vec![
        ("SATS_STATE_REPO".into(), state_repo.clone()),
        ("SATS_OPS_REPO".into(), ops_repo.clone()),
        ("SATS_SWEEP_ADDRESS".into(), bot_sweep_addr.clone()),
        ("SATS_GITHUB_TOKEN".into(), gh_pat.clone()),
        ("SATS_MEMPOOL_BASE_URL".into(), mempool_base.clone()),
        ("SATS_PRICE_PER_24H_SATS".into(), "1000".into()),
        // Long grace so the optimistic-bind reaper doesn't fire
        // mid-test if signet network conditions are slow. The reaper
        // is exercised by a separate dedicated test (TODO).
        ("SATS_OPTIMISTIC_BIND_GRACE_SECS".into(), "86400".into()),
        (
            "SATS_DD_AUTH_TOKEN".into(),
            dd_auth_token.clone().unwrap_or_default(),
        ),
    ])
    .await?;
    eprintln!("integ: bot up at {}", bot.base_url);

    // Claim + invoice.
    let create = bot
        .tool(
            "claim.create",
            json!({
                "mode": "customer_deploy",
                "customer_owner": customer,
            }),
        )
        .await?;
    let issue_number = create["issue_number"].as_u64().context("issue_number")?;
    let claim_id = create["claim"]["claim_id"]
        .as_str()
        .context("claim_id")?
        .to_string();
    eprintln!("integ: claim {claim_id} created on issue #{issue_number}");

    let invoice = bot
        .tool(
            "btc.invoice",
            json!({ "issue_number": issue_number, "blocks": 1 }),
        )
        .await?;
    assert_eq!(invoice["state"], "invoice_created");
    let amount_sats = invoice["amount_sats"]
        .as_u64()
        .context("invoice amount_sats")?;
    eprintln!("integ: invoice {amount_sats} sats → {bot_sweep_addr}");

    // Stage everything past this point inside an inner async block
    // so a bail / assertion failure still falls through to teardown
    // before propagating.
    let result = run_demo(
        &bot,
        &mut wallet,
        issue_number,
        amount_sats,
        &bot_sweep_addr,
        &dd_agent_host,
        &dd_agent_id,
        dd_auth_token.as_deref(),
        &customer,
    )
    .await;

    // Teardown: revoke the customer owner so subsequent runs / real
    // claims start from a clean dd-agent. Best-effort — never masks
    // the test result.
    if let Err(e) = teardown_revoke(&gh_pat, &ops_repo, &claim_id, &dd_agent_host).await {
        eprintln!("integ: teardown failed: {e:?}");
    } else {
        eprintln!("integ: teardown dispatched (agent_owner=\"\")");
    }

    result
}

#[allow(clippy::too_many_arguments)]
async fn run_demo(
    bot: &BotHarness,
    wallet: &mut SignetWallet,
    issue_number: u64,
    amount_sats: u64,
    bot_sweep_addr: &str,
    dd_agent_host: &str,
    dd_agent_id: &str,
    dd_auth_token: Option<&str>,
    customer: &str,
) -> Result<()> {
    // 1. Customer pays. Wallet is on signet; payment hits mempool in
    //    seconds via the mempool.space adapter.
    let txid = wallet.broadcast_exact(bot_sweep_addr, amount_sats).await?;
    eprintln!("integ: broadcast txid {txid}");

    // 2. Tick until btc_mempool_seen.
    wait_for_state(bot, issue_number, &["btc_mempool_seen"], STATE_TIMEOUT)
        .await
        .context("waiting for btc_mempool_seen")?;

    // 3. Inject the real test agent's hostname/id onto the claim. In
    //    production this is the boot-agent.yml stub's job; for the test
    //    we use a long-lived dd-agent that's already booted, so we
    //    skip the boot dispatch and just write the manifest fields.
    let claim_now = bot
        .tool("claim.load", json!({ "issue_number": issue_number }))
        .await?;
    let mut manifest = claim_now["claim"].clone();
    manifest["agent_id"] = json!(dd_agent_id);
    manifest["agent_hostname"] = json!(dd_agent_host);
    bot.tool(
        "claim.update",
        json!({
            "issue_number": issue_number,
            "claim": manifest,
            "event_note": "integ test: injected agent_id + agent_hostname",
        }),
    )
    .await?;

    // 4. Dispatch owner-update. Optimistic 0-conf path lets this fire
    //    while the tx is still in mempool — no need to wait for a
    //    signet block.
    let dispatch = bot
        .tool(
            "dd.dispatch_owner_update",
            json!({ "issue_number": issue_number }),
        )
        .await?;
    let new_state = dispatch["claim_id"].as_str(); // smoke-check shape
    eprintln!("integ: owner-update dispatched (claim_id field present: {new_state:?})");

    // 5. Tick until active. The bot's tick_owner_update_dispatched
    //    polls /health.agent_owner; once the workflow has POSTed to
    //    /owner, the next tick advances state to Active.
    wait_for_state(bot, issue_number, &["active"], STATE_TIMEOUT)
        .await
        .context("waiting for active")?;

    // 6. Spot-check the dd-agent's /health directly — the punchline.
    let health = fetch_health(dd_agent_host, dd_auth_token).await?;
    let observed = health
        .get("agent_owner")
        .and_then(Value::as_str)
        .unwrap_or("");
    if observed != customer {
        bail!("dd-agent /health.agent_owner=`{observed}`; expected `{customer}`");
    }
    eprintln!("integ: dd-agent /health.agent_owner == {customer} ✓");
    Ok(())
}

async fn wait_for_state(
    bot: &BotHarness,
    issue_number: u64,
    targets: &[&str],
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        let resp = bot
            .tool("claim.tick", json!({ "issue_number": issue_number }))
            .await?;
        let state = resp["new_state"].as_str().unwrap_or("?").to_string();
        if state != last {
            eprintln!("integ: state → {state}");
            last = state.clone();
        }
        if targets.iter().any(|t| t == &state) {
            return Ok(state);
        }
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
    bail!("timed out waiting for one of {targets:?}; last state={last}")
}

async fn fetch_health(host: &str, auth_token: Option<&str>) -> Result<Value> {
    let url = format!("https://{}/health", host.trim_end_matches('/'));
    let mut req = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(20));
    if let Some(t) = auth_token.filter(|s| !s.is_empty()) {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("GET {url} → {}", resp.status());
    }
    Ok(resp.json::<Value>().await?)
}

/// Direct workflow_dispatch on `${ops_repo}/owner-update.yml` with
/// `agent_owner=""` to revoke. We bypass the bot here because
/// `dd.dispatch_owner_update` reads `agent_owner` from
/// `claim.customer_owner`, which is the customer login (not the
/// empty string). The reaper has the same machinery internally; for
/// test teardown we just want the same effect at a known time.
async fn teardown_revoke(
    gh_pat: &str,
    ops_repo: &str,
    claim_id: &str,
    agent_host: &str,
) -> Result<()> {
    let url = format!(
        "https://api.github.com/repos/{ops_repo}/actions/workflows/owner-update.yml/dispatches"
    );
    let body = json!({
        "ref": "main",
        "inputs": {
            "claim_id": claim_id,
            "agent_host": agent_host,
            "agent_owner": "",
        }
    });
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(gh_pat)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "satsforcompute-integration-test")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    if !resp.status().is_success() {
        bail!(
            "teardown POST → {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }
    Ok(())
}
