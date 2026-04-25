---
name: satsforcompute
description: Drive the Sats for Compute marketplace bot — open claims, generate BTC invoices, dispatch agent boots. Pay sats, get attested DevOpsDefender compute.
version: 0.1.0
license: MIT-0
metadata:
  openclaw:
    homepage: https://satsforcompute.com
    emoji: "🦞"
    primaryEnv: SATS_TOOL_API_TOKEN
    requires:
      env:
        - SATS_TOOL_API_TOKEN
        - SATS_BOT_URL
      bins:
        - curl
        - jq
---

# Sats for Compute

Pay BTC, get an attested TDX VM running the
[DevOpsDefender](https://github.com/devopsdefender/dd) agent. This
skill wraps the bot's `/tools/*` HTTP API so an OpenClaw assistant can
drive the v0 product flow in natural language.

## Setup

Set two environment variables before invoking any tool below:

- `SATS_BOT_URL` — base URL of the bot (e.g.
  `https://<agent>-bot.devopsdefender.com`). Discover the current
  hostname via:

  ```bash
  curl -fsSL https://app.devopsdefender.com/api/agents \
    | jq -r '.[] | select(.vm_name=="dd-local-bot" and .status=="healthy").hostname' \
    | sed -E 's/^([^.]+)\.(.*)$/\1-bot.\2/'
  ```

- `SATS_TOOL_API_TOKEN` — the operator's tool-API bearer token. Ask
  the operator (the canonical operator runs at satsforcompute.com).

All commands below assume both are exported. They use `curl -fsSL` so
non-2xx responses fail loudly; the bot returns
`{"error": "<message>"}` on any failure.

## claim_create — Open a new claim

Open a Sats for Compute claim. Returns the manifest, GitHub issue
number, and issue URL. Two product modes:

- `customer_deploy`: customer gets `agent_owner` set on a fresh
  dd-agent — full `/deploy`, `/exec`, `/logs`, ttyd authority. Pass
  `customer_owner` (their GitHub user/org).
- `confidential`: bot deploys a sealed workload from a public GitHub
  repo (`workload_repo`); the agent has no `/owner` route, so nobody
  (operator included) can mutate the running code post-boot. The TDX
  quote proves it.

```bash
# customer-deploy
curl -fsSL -X POST "$SATS_BOT_URL/tools/claim.create" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"mode":"customer_deploy","customer_owner":"alice"}' | jq .

# confidential (sealed oracle from a public repo)
curl -fsSL -X POST "$SATS_BOT_URL/tools/claim.create" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"mode":"confidential","workload_repo":"alice/oracle","workload_ref":"v1.0"}' | jq .
```

Request:

```json
{
  "mode": "customer_deploy" | "confidential",
  "customer_owner": "<github-user-or-org>",
  "workload_repo": "<owner>/<repo>",
  "workload_ref": "<branch|tag|sha>"
}
```

Example response:

```json
{
  "claim": { "schema": "s12e.claim.v1", "claim_id": "claim_...", "state": "requested", ... },
  "issue_number": 42,
  "issue_url": "https://github.com/satsforcompute/test-claims/issues/42"
}
```

## claim_load — Fetch a claim's current state

Use before `claim_update` to round-trip the manifest, or to inspect
state mid-flow (e.g. *has the orchestrator transitioned the claim to
`node_assignment_started` yet?*).

```bash
ISSUE=42
curl -fsSL -X POST "$SATS_BOT_URL/tools/claim.load" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"issue_number\": $ISSUE}" | jq .
```

## claim_update — Write a modified manifest back

The bot validates that the schema and `claim_id` match the existing
issue, applies the new manifest, flips state-tracking labels, and
appends an event-history comment. **Send the full claim object** —
partial updates aren't supported. Use `claim_load` first to grab the
current manifest.

```bash
ISSUE=42
CLAIM=$(curl -fsSL -X POST "$SATS_BOT_URL/tools/claim.load" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"issue_number\": $ISSUE}" | jq '.claim')

# Example: force state = active for end-to-end orchestrator testing
NEW_CLAIM=$(echo "$CLAIM" | jq '.state="active" | .billing.paid_until="2027-01-01T00:00:00Z"')

curl -fsSL -X POST "$SATS_BOT_URL/tools/claim.update" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$(jq -n --argjson c "$NEW_CLAIM" --argjson n "$ISSUE" \
    '{issue_number:$n, claim:$c, event_note:"manual: smoke test"}')" | jq .
```

## btc_invoice — Generate a BIP21 BTC invoice

The customer pastes the URI into their wallet and pays; the
orchestrator polls mempool.space and transitions the claim through
`btc_mempool_seen` → `active` automatically.

Default invoice is one 24-hour block at the operator's price (50,000
sats by default). Pass `blocks > 1` for top-ups (each block extends
`paid_until` by 24h).

```bash
ISSUE=42
curl -fsSL -X POST "$SATS_BOT_URL/tools/btc.invoice" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"issue_number\": $ISSUE}" | jq .
# → { "bip21_uri": "bitcoin:bc1q...?amount=0.00050000&...", "amount_sats": 50000, ... }

# top-up: 3 blocks (72 hours)
curl -fsSL -X POST "$SATS_BOT_URL/tools/btc.invoice" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"issue_number\": $ISSUE, \"blocks\": 3}" | jq .
```

## node_boot — Manually dispatch the boot-agent workflow

The orchestrator normally calls this automatically the moment a claim
transitions to `state=active`. Use this tool only for manual retries
or operator overrides.

```bash
curl -fsSL -X POST "$SATS_BOT_URL/tools/node.boot" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"issue_number\": $ISSUE}" | jq .
```

Returns the dispatch payload (`ops_repo`, workflow filename, ref,
inputs) so you can locate the resulting workflow run on the operator's
ops repo (default `satsforcompute/sats-ops`).

## dd_dispatch_owner_update — Bind agent_owner manually

Dispatches the owner-update workflow to set `agent_owner` on the
assigned dd-agent to the customer's GitHub identity. Only valid for
`customer_deploy` claims (confidential agents don't register `/owner`).
The orchestrator does this automatically once the boot workflow has
populated `agent_hostname`; expose for manual recovery if a workflow
callback was lost.

```bash
curl -fsSL -X POST "$SATS_BOT_URL/tools/dd.dispatch_owner_update" \
  -H "Authorization: Bearer $SATS_TOOL_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"issue_number\": $ISSUE}" | jq .
```

Optional `agent_host` overrides the manifest's `agent_hostname` (useful
when the bot's view of the agent is stale).

## End-to-end demo prompt

> Open a customer-deploy claim for `alice`, give me the issue URL,
> generate a 1-block BIP21 invoice, then load the claim and tell me
> what state it's in.

The assistant should call `claim_create` → render the URL →
`btc_invoice` → render the URI → `claim_load` → report `state`. State
of record is the GitHub issue manifest at the operator's state repo
(default `satsforcompute/test-claims`).
