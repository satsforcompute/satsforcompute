# @satsforcompute/mcp

MCP server for the [Sats for Compute](https://satsforcompute.com)
marketplace bot. Drive the v0 product flow — open claims, generate BTC
invoices, dispatch agent boots — in natural language from any MCP host
(Claude Code, Cursor, Zed, claude.ai, etc.).

The server is a thin, typed shim over the bot's `/tools/*` HTTP API.
Each MCP tool maps 1:1 to a bot endpoint; auth is a bearer token the
operator hands to the customer.

## Install

No install needed — `npx @satsforcompute/mcp` is the entrypoint.

## Configure

Add to your MCP host config (the example below is Claude Code's
`~/.claude/.config.json`; Cursor and others use a similar shape):

```json
{
  "mcpServers": {
    "satsforcompute": {
      "command": "npx",
      "args": ["@satsforcompute/mcp"],
      "env": {
        "SATS_BOT_URL": "https://<bot-host>",
        "SATS_TOOL_API_TOKEN": "<your tool API token>"
      }
    }
  }
}
```

Both env vars are required; the server exits 1 with a stderr message
if either is unset.

### Discovering the bot URL

The canonical operator runs the bot on a dynamic agent hostname
(`<agent>-bot.devopsdefender.com`). To find the current value:

```bash
curl -fsSL https://app.devopsdefender.com/api/agents \
  | jq -r '.[] | select(.vm_name=="dd-local-bot" and .status=="healthy").hostname' \
  | sed -E 's/^([^.]+)\.(.*)$/\1-bot.\2/'
```

A stable CNAME is planned as a follow-up.

## Tools

| Tool | Purpose |
|---|---|
| `claim_create` | Open a new claim (customer-deploy or confidential mode) |
| `claim_load` | Fetch a claim manifest by GitHub issue number |
| `claim_update` | Write a modified manifest back; flips state labels + appends event comment |
| `btc_invoice` | Generate a BIP21 BTC invoice URI |
| `node_boot` | Dispatch the boot-agent workflow (manual override; orchestrator does this automatically) |
| `dd_dispatch_owner_update` | Dispatch the owner-update workflow (customer-deploy only) |

## Demo

Once configured, ask the LLM things like:

> "Open a customer-deploy claim for posix4e and show me the issue URL."

> "Generate a BIP21 invoice for claim 7 — three blocks for a 72-hour rental."

> "Load claim 12 and tell me whether the orchestrator has booted an agent yet."

> "Flip claim 5 to active so the orchestrator dispatches boot-agent.yml — useful for end-to-end testing without paying real BTC."

The LLM picks the matching tool, the server forwards to the bot, and
the JSON response shows up in the chat. State of record stays in
GitHub issues on the operator's state repo (default
`satsforcompute/test-claims`).

## Develop

```bash
npm install
npm run build
npm test
```

## License

MIT.
