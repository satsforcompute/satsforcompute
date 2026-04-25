# Sats for Compute

Pay BTC, get attested DevOpsDefender compute.

The operator-side bot for a self-service node marketplace. A user sends bitcoin and receives control (or a sealed workload, see below) of an attested TDX VM running the [DevOpsDefender](https://github.com/devopsdefender/dd) agent.

## Status

**v0 scaffold.** This branch is a full rewrite of the previous `dd-market` iteration (BTCPay + SQLite + SSH-provisioning). The new design is BDK-in-enclave for wallet, GitHub issues for state, GitHub Actions OIDC + `workflow_dispatch` for privileged DD mutations.

The spec lives at [`SATS_FOR_COMPUTE_SPEC.md` in `devopsdefender/dd`](https://github.com/devopsdefender/dd/blob/main/SATS_FOR_COMPUTE_SPEC.md). This crate implements the operator-side tool server + state model described there. Real handlers land in subsequent PRs; v0 ships the listener + config + the canonical `Claim` manifest schema (`s12e.claim.v1`) so the deploy pipeline has something to wire up.

## Two product modes

- **Customer-deploy mode** (default): customer pays, gets `DD_AGENT_OWNER` set on a fresh dd-agent. Full `/deploy` / `/exec` / `/logs` / ttyd authority, alongside the fleet operator. Intended for general-purpose compute.
- **Confidential mode**: customer specifies a public GitHub repo containing a `workload.json`. Bot deploys the workload onto a dd-agent booted with `DD_CONFIDENTIAL=true` — the agent doesn't even register `/deploy` / `/exec` / `/owner` routes. Nobody, including the operator, can mutate the running workload post-boot. `/logs` and attestation stay open. Use case: oracles / bot-oracles where the operator proves the code is sealed.

The two modes are different boot configurations of the same dd-agent; attestation already measures the boot config, so a third party can verify which mode a node is in directly from its TDX quote.

## Build

```bash
cargo fmt --all
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

`/healthz` echoes the static config back so deploy verification can confirm the binary is up with the right secrets baked in.

## Website

The customer-facing landing at <https://satsforcompute.com> is served from the
[`gh-pages`](https://github.com/satsforcompute/satsforcompute/tree/gh-pages)
branch. Edit `index.html` / `style.css` there; PRs against `gh-pages` get a
preview at `satsforcompute.com/pr-preview/<N>/` via `rossjrw/pr-preview-action`.
Don't bake the marketing site into this Rust crate — it lives on its own
branch so design changes don't churn the bot's commit history.

## Forking your own operator

The bot is deliberately operator-local — every forking operator runs their own instance with their own state repo, sweep address, and (optionally) trust policy on the `/owner` callback. Spec section "Forkable example" covers what a fork looks like. v0 is single-canonical at `satsforcompute.com`; multi-operator federation is post-v0.

## License

MIT
