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
`agent_hostname` on the manifest. The workflow is responsible for:

1. Calling DD's `/owner` endpoint on the agent (or the equivalent
   workflow_dispatch flow) so `agent_owner = customer_owner` —
   transferring `/deploy` / `/exec` / `/logs` / ttyd authority to the
   customer's GitHub identity.
2. Calling `claim.update` on the bot to set `state = active` once the
   binding is confirmed.

Inputs:

| input          | example                              |
| ---            | ---                                  |
| `claim_id`     | `claim_1745678901_a1b2c3d4`          |
| `agent_host`   | `dd-agent-7.devopsdefender.com`      |
| `agent_owner`  | `alice`                              |

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

## State-ownership wart (v0)

The orchestrator owns most state transitions, but `owner-update.yml`
is asked to write `state = active` itself once the binding lands.
That's because the orchestrator can't observe agent owner state from
the manifest alone; it would need to query the agent's `/health`. For
v0 the workflow does it. A future change can move this to a polling
loop on `/health.agent_owner` and revert the workflow to data-only.

## Worked example: canonical operator (`satsforcompute/sats-ops`)

The canonical operator's stub workflows ship at
[`satsforcompute/sats-ops`](https://github.com/satsforcompute/sats-ops):

- `boot-agent.yml` — for the demo, echoes inputs and posts a stub
  `agent_id`/`agent_hostname` back via `claim.update`. Real
  provisioning is operator-specific; the stub is enough to drive the
  state machine end-to-end.
- `owner-update.yml` — same pattern; echoes inputs and writes
  `state = active` back via `claim.update`.

Production operators replace the bodies with real provisioning +
real DD `/owner` calls. The contract above doesn't change.

## Auth from the workflow back to the bot

Workflows call back into the bot's tool API with the operator's
`SATS_TOOL_API_TOKEN`. Set it as a secret on the ops repo and pass it
as a header on the `claim.update` request. The bot doesn't validate
*which* workflow is calling — only that the token matches — so treat
the ops-repo's secrets surface as part of your trust boundary.
