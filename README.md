# Sats for Compute

Pay BTC, get attested DevOpsDefender compute.

The operator-side bot for a self-service node marketplace. A customer sends bitcoin and receives control of an attested TDX VM running the [DevOpsDefender](https://github.com/devopsdefender/dd) agent — or, in confidential mode, a sealed workload nobody including the operator can mutate.

The spec lives at [`SATS_FOR_COMPUTE_SPEC.md` in `devopsdefender/dd`](https://github.com/devopsdefender/dd/blob/main/SATS_FOR_COMPUTE_SPEC.md).

## Shape

A small HTTP tool server. Seven tools (`claim.create`, `btc.invoice`, `claim.tick`, `claim.load`, `claim.update`, `node.boot`, `dd.dispatch_owner_update`) drive a 7-state claim machine over GitHub issues. State transitions only happen when a tool is called — no background loop, no local DB. LLM frontends or external schedulers decide when to tick.

## Two product modes

- **Customer-deploy** (default): customer pays, the bot sets `agent_owner` on a fresh TDX dd-agent via `repository_dispatch` → operator-ops workflow → GitHub Actions OIDC → `POST /owner`. Customer gets full `/deploy` / `/exec` / `/logs` authority alongside the operator.
- **Confidential**: customer specifies a public GitHub repo containing `workload.json`. Bot deploys onto an agent booted with `DD_CONFIDENTIAL=true` — the agent has no `/deploy` / `/exec` / `/owner` routes. Nobody, including the operator, can mutate the workload post-boot. `/logs` and attestation stay open.

The two modes are different boot configs of the same dd-agent; the TDX quote measures the boot config, so a third party can verify the mode directly.

## Build

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

`/healthz` echoes the static config so a deploy verifier can confirm the binary is up with the right secrets baked in.

## Integration test

A real-bot + real-BTC + real-GitHub + real-dd-agent integration test lives in `tests/integration_signet.rs`. Local-only, gated:

```bash
SIGNET_SMOKE=1 cargo test --test integration_signet -- --ignored --nocapture
```

See `CLAUDE.md` for the full env-var list and one-time wallet setup. **Self-driving** — a BDK signet wallet baked into the test broadcasts the customer's payment, drives the optimistic-0-conf path through `active`, asserts dd-agent `/health.agent_owner` flipped to the test customer, then revokes the binding in teardown. No fakes, no mocks, no manual faucet drip between runs once the seed is funded once.

## Website

The customer-facing landing at <https://satsforcompute.com> is served from the
[`gh-pages`](https://github.com/satsforcompute/satsforcompute/tree/gh-pages)
branch. Edit `index.html` / `style.css` there; PRs against `gh-pages` get a
preview at `satsforcompute.com/pr-preview/<N>/` via `rossjrw/pr-preview-action`.
Don't bake the marketing site into this Rust crate — it lives on its own
branch so design changes don't churn the bot's commit history.

## Forking your own operator

The bot is deliberately operator-local — every forking operator runs their own instance with their own state repo, ops repo, sweep address, and (optionally) trust policy on the `/owner` callback.

## License

MIT
