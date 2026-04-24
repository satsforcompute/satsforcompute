# satsforcompute (rewrite — v0)

Pay sats, get attested DevOpsDefender compute. Operator-side bot.

This repo is a full rewrite of the previous BTCPay+SQLite+SSH iteration. The spec lives at `SATS_FOR_COMPUTE_SPEC.md` in `devopsdefender/dd`. v0 = scaffold + `Claim` manifest schema (`s12e.claim.v1`); real handlers land in follow-up PRs.

## Architecture (target — most pieces are stubs in v0)

- **Tool server (this crate)** — a constrained tool/policy layer. LLM frontends (OpenClaw, Codex, OpenAI Agents SDK, custom UI) call these tools, never raw Cloudflare/GitHub/BTC APIs. Tools enforce policy even when an LLM picks the next action.
- **State backend = GitHub issues.** One claim per issue. The body is the canonical JSON manifest; comments are append-only event history. No local DB.
- **Privileged DD actions = `workflow_dispatch`.** The bot triggers an operator-ops workflow that mints GitHub Actions OIDC and calls `POST /owner` / `/deploy` on dd-agents. The bot itself never holds DD write credentials.
- **Wallet = BDK in a separate EE workload.** Ephemeral hot seed, sweeps to operator cold storage on every 1-conf payment. Operator restart is an accepted funds-loss event for in-flight invoices.
- **BTC watcher = pluggable (mempool.space adapter for v0).** Polled.

## Key types

- `claim::Claim` — canonical manifest (`s12e.claim.v1`). Both modes use it; `mode: customer_deploy | confidential` picks the boot shape on the assigned dd-agent.
- `claim::Integrity` — mirrors dd-agent's `/health.taint_reasons` + `/health.confidential_mode`. Cached so claim consumers don't need a live agent fetch.

## Build & test

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --target x86_64-unknown-linux-musl -- -D warnings
cargo test --workspace --target x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

## Run locally

```bash
SATS_STATE_REPO=myorg/s4c-ops \
SATS_SWEEP_ADDRESS=bc1q-replace-me \
RUST_LOG=satsforcompute=debug \
cargo run
```

## Required env (production)

| Var | Notes |
|---|---|
| `SATS_STATE_REPO` | `owner/repo` for claim issues. Public for demos, private for prod. |
| `SATS_SWEEP_ADDRESS` | Operator's BTC cold-storage address. Stored as a GitHub secret in the operator-ops repo, populated into the enclave's config disk at deploy. |
| `SATS_PORT` | Default 8080. |
| `SATS_PRICE_PER_24H_SATS` | Default 50000 (~$30/day at current BTC). |
| `SATS_PENDING_TIMEOUT_SECS` | Default 10800 (3h). Pending-payment grace window before reclaim. |
| `SATS_DD_CP_URL` | Default `https://app.devopsdefender.com`. |

## Spec questions answered (live during the rewrite review)

The full Q&A is captured in `~/.claude/plans/lets-come-up-with-squishy-goose.md` (operator-local). Headlines:

- Single canonical operator at `satsforcompute.com` for v0.
- Operator picks own cold-storage; spec doesn't prescribe identity.
- Confidential-mode workload comes from a customer-provided public GitHub repo with `workload.json` at root.
- 50k sats / 24h is the real starting price.
- No rate-limiting in v0 — economic model self-limits.
- Manual-review refund path on node failure.
- GitHub issue filters ARE the operator dashboard (no separate UI in v0).

## Out of scope for this PR

- Real handlers for `claim.create` / `btc.invoice` / `node.boot` / etc. — follow-up PRs.
- BDK wallet workload — separate enclave repo.
- OpenClaw config — separate PR once tools land.
- LICENSE file — README claims MIT but no formal LICENSE file; add when convenient.
