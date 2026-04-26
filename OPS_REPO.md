# Ops repo contract

This crate dispatches privileged actions to a separate **operator-ops
repo** via `workflow_dispatch`. The bot itself never holds DevOpsDefender
write credentials or cloud-provider creds; it just emits typed events.
The ops repo is what actually boots VMs, sets `agent_owner`, and reaps
expired claims.

This is the spec section "GitHub Actions As Actuator" implemented.
Forking operators replace the canonical `satsforcompute/sats-ops` with
their own repo and configure `SATS_OPS_REPO` accordingly.

## Where this is configured

```bash
SATS_OPS_REPO=satsforcompute/sats-ops      # owner/repo of your ops repo
SATS_OPS_BOOT_WORKFLOW=boot-agent.yml      # default
SATS_OPS_OWNER_WORKFLOW=owner-update.yml   # default
SATS_OPS_REF=main                          # default
```

## Workflows the ops repo MUST provide

### `boot-agent.yml`

Dispatched when a claim transitions `active` (no agent yet) → the bot
needs a fresh dd-agent. The workflow is responsible for:

1. Provisioning a confidential VM (cloud-side, however you do it).
2. Booting the dd-agent on it. For confidential-mode claims, set
   `DD_CONFIDENTIAL=true` on the agent so `/deploy`/`/exec`/`/owner`
   are not registered, and deploy the customer's `workload.json`
   from `workload_repo` @ `workload_ref`.
3. Calling `claim.update` on the bot with `agent_id` and
   `agent_hostname` populated. **Do not change `state`** — the
   orchestrator owns state transitions.

Inputs (all strings; coerce as needed inside the workflow):

| input            | example                          | required when                |
| ---              | ---                              | ---                          |
| `claim_id`       | `claim_1745678901_a1b2c3d4`      | always                       |
| `mode`           | `customer_deploy` / `confidential` | always                     |
| `customer_owner` | `alice`                          | customer_deploy mode         |
| `workload_repo`  | `alice/my-oracle`                | confidential mode            |
| `workload_ref`   | `v1.2.3` / `main`                | confidential mode (defaults `main`) |

### `owner-update.yml`

Dispatched after a customer-deploy claim's boot workflow has populated
`agent_hostname` on the manifest. The workflow is responsible for one
thing:

- Mint a GitHub Actions OIDC token and POST it to the dd-agent's
  `/owner` endpoint to set `agent_owner = customer_owner` (or to clear
  it on revoke — see below). dd-agent's `set_owner` is fleet-gated, so
  the workflow's repo MUST be a trusted issuer for the dd fleet
  (`DD_OWNER`).

The workflow does **not** write claim state back. The bot's
`tick_owner_update_dispatched` polls `/health.agent_owner` and
advances `OwnerUpdateDispatched → Active` itself once the binding
lands. (Earlier v0 had the workflow flip state directly; that's
resolved.)

Inputs:

| input          | example                              |
| ---            | ---                                  |
| `claim_id`     | `claim_1745678901_a1b2c3d4`          |
| `agent_host`   | `dd-agent-7.devopsdefender.com`      |
| `agent_owner`  | `alice` — or empty string to revoke  |

`agent_owner=""` is a valid input: it tells the workflow to clear the
runtime tenant (POST `/owner` with empty body to dd-agent, which
resets `agent_owner` to `None` so only the fleet operator can call
`/deploy`/`/exec`). The bot dispatches with empty `agent_owner` when
its optimistic-bind reaper fires, so the workflow MUST handle that
case rather than failing on missing-input validation.

## Idempotency contract

The bot's transition gates ensure a successful dispatch advances the
claim's state, so a re-dispatch of the *same* `claim_id` doesn't
normally happen. But there's a small window: dispatch_workflow
succeeds → `claim.update` (the bot's state-write) fails → next tick
re-dispatches. To handle that cleanly:

- **`boot-agent.yml` MUST be idempotent on `claim_id`.** Re-running
  with a `claim_id` that's already provisioned should re-set the same
  `agent_id`/`agent_hostname` rather than provision a second VM.
- **`owner-update.yml` MUST be idempotent on `claim_id`.** Setting
  `agent_owner` to the value it already has must succeed.

In practice: query your provisioning system by `claim_id` first; act
only if no node exists yet.

## Worked example: canonical operator (`satsforcompute/sats-ops`)

The canonical operator's stub workflows ship at
[`satsforcompute/sats-ops`](https://github.com/satsforcompute/sats-ops):

- `boot-agent.yml` — for the demo, echoes inputs and posts a stub
  `agent_id`/`agent_hostname` back via `claim.update`. Real
  provisioning is operator-specific; the stub is enough to drive the
  state machine end-to-end.
- `owner-update.yml` — mints a GitHub Actions OIDC token (audience
  `dd-agent`) and POSTs to the target dd-agent's `/owner` endpoint.
  No callback to the bot; `claim.tick` advances `OwnerUpdateDispatched
  → Active` once `/health.agent_owner` reflects the binding.

Production operators replace `boot-agent.yml` with real cloud
provisioning. `owner-update.yml` is already real.

## Auth from the workflow back to the bot

Workflows call back into the bot's tool API with the operator's
`SATS_TOOL_API_TOKEN`. Set it as a secret on the ops repo and pass it
as a header on the `claim.update` request. The bot doesn't validate
*which* workflow is calling — only that the token matches — so treat
the ops-repo's secrets surface as part of your trust boundary.
