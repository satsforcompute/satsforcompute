# satsforcompute

Pay sats, get attested DevOpsDefender compute. Operator-side bot.

This repo was a full rewrite of the previous BTCPay+SQLite+SSH iteration. It then grew speculative scaffolding (a 17-state lifecycle enum, a stub background orchestrator, in-memory test fakes, a regtest backend) that wasn't pulling its weight. **Stripped to the bone:** a pure tool/policy layer over the canonical `Claim` manifest schema. No mocks, no regtest — the only BTC backend is the real `mempool.space` adapter, and the integration test runs against signet.

The spec lives at `SATS_FOR_COMPUTE_SPEC.md` in `devopsdefender/dd`.

## Architecture

- **Tool server (this crate)** — every state transition fires from an explicit `POST /tools/<name>` call. No background loop. LLM frontends (OpenClaw, Codex, custom UIs) drive it; an external scheduler (cron, the integration test, an LLM session) decides when to tick.
- **State backend = GitHub issues.** One claim per issue. The body is the canonical JSON manifest (`s12e.claim.v1`); comments are append-only event history. No local DB.
- **Privileged DD actions = `workflow_dispatch`.** `node.boot` and `dd.dispatch_owner_update` fire workflows on the operator-ops repo (defaults: `boot-agent.yml`, `owner-update.yml`); those workflows mint GitHub Actions OIDC and call `POST /owner` / boot the VM. The bot never holds DD write credentials. See `OPS_REPO.md` for the workflow contract.
- **dd-agent reachability = Cloudflare zero-trust.** Agents register with dd-cp on boot; CF fronts them at a public URL. The bot polls `/health` through that URL (optional bearer via `SATS_DD_AUTH_TOKEN`).
- **BTC watcher = `mempool.space` REST.** Single backend; mainnet by default, signet via `SATS_MEMPOOL_BASE_URL`. `BtcWatcher` is still a trait so the caller doesn't reach into adapter internals, but there is no second impl — no fakes, no regtest.
- **Wallet = static sweep address + per-claim LSD signature (v0 stub).** Every claim invoices into the operator's single `SATS_SWEEP_ADDRESS`. To attribute payments without per-claim addresses, `claim.create` bakes a 1..=9999 sat perturbation into `BtcDetails.exact_amount_sats`; the customer pays exactly that amount and the watcher matches by amount. BDK enclave wallet for true per-claim derivation lands later.
- **Optimistic 0-conf bind, with a 1-hour reaper.** `dd.dispatch_owner_update` fires as soon as the bot sees the tx in mempool (per spec §"optimistic 0-conf for initial access"). If the tx hasn't reached `required_confirmations` within `SATS_OPTIMISTIC_BIND_GRACE_SECS` (default 3600), `claim.tick` reaps the claim: dispatches owner-update with an empty `agent_owner` to revoke access, transitions state to `failed`, and surfaces the event for manual review.
- **End-of-block reaper.** When `dd.dispatch_owner_update` advances the claim, the bot stamps `Billing.paid_until = now + 24h`. `claim.tick` on `Active` checks `paid_until` and, if elapsed, dispatches owner-update with an empty `agent_owner` to revoke and transitions state to `failed`. v0 = single 24h block; multi-block top-ups are out of scope.

## Tools

| Tool | Effect |
|---|---|
| `claim.create` | Open a new claim issue, bake the per-claim LSD signature into `exact_amount_sats`. State: `requested`. |
| `btc.invoice` | Return a BIP21 URI for `exact_amount_sats × blocks`. State: `requested` → `invoice_created`. |
| `claim.tick` | Advance state from BTC observations + dd-agent `/health`. Also runs the optimistic-bind reaper for `owner_update_dispatched` / `active`. |
| `claim.load` | Fetch the canonical manifest from GitHub. |
| `claim.update` | Manual-override write of a manifest. |
| `node.boot` | `workflow_dispatch` on `boot-agent.yml` to provision a fresh dd-agent. |
| `dd.dispatch_owner_update` | `workflow_dispatch` on `owner-update.yml`. State: `btc_mempool_seen` (optimistic 0-conf) or `btc_confirmed` → `owner_update_dispatched`. |

State machine (post-strip): `requested` → `invoice_created` → `btc_mempool_seen` → `btc_confirmed` → `owner_update_dispatched` → `active` (plus terminal `failed`). The 0-conf path skips `btc_confirmed` directly into `owner_update_dispatched` and relies on the reaper to fail-close if the tx never settles.

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
SATS_OPS_REPO=myorg/s4c-ops \
SATS_SWEEP_ADDRESS=bc1q-replace-me \
SATS_GITHUB_TOKEN=ghp_... \
SATS_TOOL_API_TOKEN=secret \
RUST_LOG=satsforcompute=debug \
cargo run
```

## Required env (production)

| Var | Notes |
|---|---|
| `SATS_STATE_REPO` | `owner/repo` for claim issues. |
| `SATS_OPS_REPO` | `owner/repo` for `repository_dispatch`. Defaults to `state_repo` if unset. |
| `SATS_SWEEP_ADDRESS` | Operator's BTC invoice address. v0 stub: shared by every claim. |
| `SATS_GITHUB_TOKEN` | PAT or App installation token. R+W on `state_repo` issues + `ops_repo` dispatches. |
| `SATS_TOOL_API_TOKEN` | Bearer token gating `/tools/*`. |
| `SATS_PORT` | Default 8080. |
| `SATS_PRICE_PER_24H_SATS` | Default 50000. |
| `SATS_PENDING_TIMEOUT_SECS` | Default 10800. Carried per spec `s12e.claim.v1`; this rewrite has no autonomous reclaimer reading it. |
| `SATS_DD_CP_URL` | Default `https://app.devopsdefender.com`. |
| `SATS_DD_AUTH_TOKEN` | Optional bearer for dd-agent `/health` (Cloudflare service token). |
| `SATS_MEMPOOL_BASE_URL` | Default `https://mempool.space/api`. Set to `.../signet/api` for signet. |
| `SATS_OPTIMISTIC_BIND_GRACE_SECS` | Default 3600 (1h). Reaper window after a 0-conf bind. |

## Integration test

`tests/integration_signet.rs` is the single end-to-end check. **Self-driving** — a BDK signet wallet broadcasts the customer's payment of exactly `BtcDetails.exact_amount_sats`. No fakes, no mocks, no manual faucet drip between runs once the seed is funded once.

Local-only, gated:

```bash
SIGNET_SMOKE=1 \
SATS_TEST_GH_PAT=ghp_... \
SATS_TEST_STATE_REPO=myorg/s4c-test \
SATS_TEST_OPS_REPO=myorg/s4c-ops-test \
SATS_TEST_CUSTOMER_OWNER=alice \
SATS_TEST_DD_AGENT_HOST=dd-local-bot.devopsdefender.com \
SATS_TEST_DD_AGENT_ID=dd-local-bot \
SATS_TEST_SIGNET_DESCRIPTOR='wpkh(tprv8.../84h/1h/0h/0/*)' \
SATS_TEST_SIGNET_CHANGE_DESCRIPTOR='wpkh(tprv8.../84h/1h/0h/1/*)' \
cargo test --test integration_signet -- --ignored --nocapture
```

Optionally: `SATS_TEST_DD_AUTH_TOKEN` (CF Access bearer for `/health`).

### One-time wallet setup

1. Generate a signet wallet (Sparrow → New Wallet → Signet, or `bdk-cli`, or `bitcoin-cli createwallet` against a signet node).
2. Export the external + change descriptors. For BIP84 segwit on signet they look like `wpkh(tprv8.../84h/1h/0h/0/*)` and the `/1/*` variant.
3. Hit a signet faucet for the wallet's first receive address with enough sats to cover many test runs (1M sats covers ~1000 runs at 1k sats per claim + LSD perturbation).
4. Set the descriptors as repo secrets (`SATS_TEST_SIGNET_DESCRIPTOR`, `SATS_TEST_SIGNET_CHANGE_DESCRIPTOR`) on the satsforcompute repo if you want CI to run the test, or in your local env for ad-hoc runs.

The test handles "wallet has zero sats" gracefully — first run with an unfunded descriptor prints the address to fund and bails loud.

### What the test does

1. Sync the BDK wallet from `https://mempool.space/signet/api`.
2. Derive a fresh receive address; hand it to the bot as `SATS_SWEEP_ADDRESS`.
3. `claim.create` → `btc.invoice`. Read `amount_sats` (price + per-claim LSD).
4. Wallet broadcasts a tx of exactly that amount to the bot's address.
5. Tick until `btc_mempool_seen`.
6. `claim.update` to inject the real test agent's `agent_id` + `agent_hostname` (we skip the boot-agent stub since the test uses a long-lived dd-agent).
7. `dd.dispatch_owner_update` (optimistic 0-conf path).
8. Tick until `active`.
9. Fetch `https://${dd_agent_host}/health` directly and assert `agent_owner == customer`.
10. **Teardown** (always runs): direct GitHub API dispatch of `owner-update.yml` with `agent_owner=""` to revoke the binding so subsequent runs / real claims start clean.

## Out of scope

- BDK enclave wallet for the **operator** (per-claim BTC addresses on the production sweep side). The integration test uses BDK only as a customer-side test wallet; the operator's sweep is still a single static address with LSD-signature attribution.
- Multi-block top-ups (current matcher requires `received_sats == exact_amount_sats` exactly).
- LICENSE file — README claims MIT.
