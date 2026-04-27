# Deploy

Operator-side runbook for standing up the satsforcompute bot at a
canonical URL (e.g. `bot.satsforcompute.com`) on a real TDX
dd-agent. Production shape: the bot runs as a sealed dd workload on
its own dd-agent (`dd-local-bot`), CF-fronted.

## Prerequisites

- A dd fleet you control (dd-cp deployed, agent images published).
  The canonical fleet is `app.devopsdefender.com`.
- `dd-local-bot` provisioned and running on a TDX host. One-time:
  ```bash
  # in the dd repo
  apps/_infra/local-agents.sh "" "" https://app.devopsdefender.com
  virsh start dd-local-bot
  ```
- `dd-local-bot` has been one-time-bound to this org via a fleet-OIDC
  POST to `/owner`. Use [dd's `set-agent-owner.yml`](https://github.com/devopsdefender/dd/blob/main/.github/workflows/set-agent-owner.yml):
  ```bash
  gh workflow run set-agent-owner.yml --repo devopsdefender/dd \
    -f cp-url=https://app.devopsdefender.com \
    -f vm-name=dd-local-bot \
    -f agent-owner=satsforcompute
  ```
  This is the trust pre-req. Without it, `Deploy bot` (and
  `sats-ops/owner-update.yml`) get rejected by `require_fleet_oidc`.
- `gh` CLI authenticated on a session that can write to
  `satsforcompute/satsforcompute` and `satsforcompute/sats-ops`.

## 1. Configure secrets on the satsforcompute repo

`Deploy bot` (`.github/workflows/deploy-bot.yml`) reads these to bake
the workload manifest. Set them as repo secrets:

```bash
REPO=satsforcompute/satsforcompute

gh secret set SATS_STATE_REPO     --repo "$REPO" --body 'satsforcompute/<state-repo>'
gh secret set SATS_OPS_REPO       --repo "$REPO" --body 'satsforcompute/sats-ops'
gh secret set SATS_SWEEP_ADDRESS  --repo "$REPO" --body 'bc1q…'  # mainnet, or tb1q… for signet
gh secret set SATS_GITHUB_TOKEN   --repo "$REPO" --body '<PAT or app token; r+w issues on state_repo, actions:write on ops_repo>'
gh secret set SATS_TOOL_API_TOKEN --repo "$REPO" --body "$(openssl rand -hex 32)"
```

The state repo can be public for demos and private for prod —
either way the bot's `SATS_GITHUB_TOKEN` needs read+write on its
issues. The ops repo must be the one with `boot-agent.yml` /
`owner-update.yml` / `claim-tick-cron.yml` (i.e. `sats-ops`).

## 2. Run a release

`Deploy bot` triggers on `workflow_run` after `Release` succeeds.
Cut a release:

```bash
# build + tag + push, or use whatever your release workflow expects
git tag v0.1.0
git push origin v0.1.0
gh workflow run release.yml --repo "$REPO"   # if release is workflow_dispatch
```

`Release` builds the static-musl binary, attaches it as the
`satsforcompute` asset to the GitHub release. `Deploy bot` then
auto-fires.

(For ad-hoc redeploys without a code change: `gh workflow run
deploy-bot.yml --repo "$REPO"`.)

## 3. Verify the deployment

`Deploy bot` calls dd's `dd-deploy` composite action, which uses
GitHub OIDC to authenticate to dd-cp and pushes the workload onto
`dd-local-bot`. After it succeeds:

```bash
# Pick the assigned hostname from dd-cp.
gh api "https://app.devopsdefender.com/api/agents" \
  | jq -r '.[] | select(.vm_name=="dd-local-bot") | .hostname'

# The bot's expose label is "bot"; the URL is
# https://bot.<dd-local-bot-hostname>/healthz
curl -fsSL "https://bot.${AGENT_HOSTNAME}/healthz" | jq
```

`/healthz` echoes the static config back. Confirm `state_repo`,
`ops_repo`, `mempool_base_url` are the values you expect.

## 4. CNAME `bot.satsforcompute.com`

To get a stable customer-facing URL:

1. In your DNS (Cloudflare, route53, etc.), add a `CNAME`:
   ```
   bot.satsforcompute.com  CNAME  bot.<dd-local-bot-hostname>
   ```
2. If the dd CF tunnel is on a Cloudflare zone you also control,
   you can use the same CF zone — see dd's CF tunnel docs.
3. `curl -fsSL https://bot.satsforcompute.com/healthz` to verify.

## 5. Configure `sats-ops`

The bot is up; now wire the cron + workflow callbacks to it:

```bash
OPS=satsforcompute/sats-ops
BOT_TOKEN="$(gh secret list --repo satsforcompute/satsforcompute | grep SATS_TOOL_API_TOKEN)"  # value: paste the same token you set in §1

gh variable set BOT_URL    --repo "$OPS" --body 'https://bot.satsforcompute.com'
gh variable set STATE_REPO --repo "$OPS" --body 'satsforcompute/<state-repo>'
gh secret   set BOT_TOOL_API_TOKEN --repo "$OPS" --body "<same token as §1>"
gh secret   set STATE_GITHUB_TOKEN --repo "$OPS" --body '<PAT with read on state_repo issues>'
```

The cron then has everything it needs. First `claim-tick-cron` run
fires within 5 minutes; verify:

```bash
gh run list --repo "$OPS" --workflow=claim-tick-cron.yml --limit 5
```

A green run with no open claims says: "no open claims to tick".

## 6. Smoke-test the full path (signet)

See `CLAUDE.md` § Integration test for the env layout. Quick
version, with the bot already deployed:

```bash
SIGNET_SMOKE=1 \
SATS_TEST_GH_PAT=$(gh auth token) \
SATS_TEST_STATE_REPO=satsforcompute/<test-state-repo> \
SATS_TEST_OPS_REPO=satsforcompute/<test-ops-repo> \
SATS_TEST_CUSTOMER_OWNER=$(gh api user --jq .login) \
SATS_TEST_DD_AGENT_HOST=dd-local-bot.<your-dd-zone> \
SATS_TEST_DD_AGENT_ID=dd-local-bot \
SATS_TEST_SIGNET_DESCRIPTOR='wpkh(tprv8.../84h/1h/0h/0/*)' \
SATS_TEST_SIGNET_CHANGE_DESCRIPTOR='wpkh(tprv8.../84h/1h/0h/1/*)' \
cargo test --test integration_signet -- --ignored --nocapture
```

Greens means the demo is live.

## Troubleshooting

- **`Deploy bot` red on `dd-deploy`**: usually `agent_owner` on
  `dd-local-bot` doesn't match this repo's org. Re-run dd's
  `set-agent-owner.yml` with `agent-owner=satsforcompute`.
- **`/healthz` 404**: the workload didn't deploy. Look at the
  `dd-deploy` step's logs.
- **`claim-tick-cron` always failing red**: probably `BOT_URL` typo
  or the bot is down. The cron's last step prints `all N
  claim.tick calls failed; bot may be unreachable`.
- **Owner-update workflow rejected at `/owner`**: the workflow's
  OIDC token doesn't match `DD_OWNER` on the agent. Same fix as
  the `dd-deploy` red — re-run `set-agent-owner.yml`.
- **Test wallet `bails` with "fund this address"**: first run with
  a fresh signet descriptor. The test prints the address; hit a
  signet faucet (signetfaucet.com) for ≥ 100k sats, re-run.
