// Six MCP tool definitions, one per /tools/* endpoint on the bot.
//
// Tool names are snake_case (MCP convention) but map to the bot's
// dotted paths (claim.create, btc.invoice, etc.). Descriptions are
// written to the LLM that's selecting tools — they're verbose on
// purpose so the model picks the right call from natural-language
// intent like "give me an invoice for claim 7."

import { z } from "zod";
import type { BotClient } from "./client.js";

export interface ToolDef {
  name: string;
  description: string;
  inputSchema: z.ZodTypeAny;
  /** Bot path the tool POSTs to. */
  path: string;
}

export const TOOLS: ToolDef[] = [
  {
    name: "claim_create",
    description:
      "Open a new Sats for Compute claim. Returns the claim manifest, the GitHub issue number that tracks it, and the issue URL. " +
      "For `customer_deploy` mode, supply the customer's GitHub user/org as `customer_owner` — they'll get `/deploy` + `/exec` + ttyd authority on the assigned dd-agent. " +
      "For `confidential` mode, supply `workload_repo` (owner/repo) holding a public `workload.json`; the bot deploys a sealed agent with no `/owner` route, and `workload_ref` (default `main`) pins the exact ref so a third party can verify the TDX measurement.",
    inputSchema: z.object({
      mode: z.enum(["customer_deploy", "confidential"]),
      customer_owner: z.string().optional(),
      workload_repo: z.string().optional(),
      workload_ref: z.string().optional(),
    }),
    path: "/tools/claim.create",
  },
  {
    name: "claim_load",
    description:
      "Fetch the current claim manifest by GitHub issue number. " +
      "Use this before `claim_update` to round-trip the manifest, or to inspect state mid-flow (e.g. has the orchestrator transitioned the claim to `node_assignment_started` yet?).",
    inputSchema: z.object({
      issue_number: z.number().int().positive(),
    }),
    path: "/tools/claim.load",
  },
  {
    name: "claim_update",
    description:
      "Write a modified claim manifest back to its GitHub issue. The bot validates that the schema and `claim_id` match the existing issue, applies the new manifest, flips state-tracking labels, and appends an event-history comment. " +
      "Common uses: reporting back from an ops-repo workflow (set `agent_id`/`agent_hostname` after boot), or operator overrides (force `state` to `manual_review`). " +
      "Pass the full claim object — partial updates aren't supported. Get the current manifest from `claim_load` first.",
    inputSchema: z.object({
      issue_number: z.number().int().positive(),
      claim: z.record(z.unknown()),
      event_note: z.string().optional(),
    }),
    path: "/tools/claim.update",
  },
  {
    name: "btc_invoice",
    description:
      "Generate a BIP21 BTC invoice URI for a claim. The customer pastes the URI into their wallet and pays; the orchestrator polls mempool.space and transitions the claim through `btc_mempool_seen` → `active` automatically. " +
      "Default invoice is one 24-hour block at the operator's configured price; pass `blocks > 1` for top-ups (each block extends `paid_until` by 24h).",
    inputSchema: z.object({
      issue_number: z.number().int().positive(),
      blocks: z.number().int().positive().optional(),
    }),
    path: "/tools/btc.invoice",
  },
  {
    name: "node_boot",
    description:
      "Dispatch the operator-ops repo's boot-agent workflow for this claim. The orchestrator normally calls this automatically the moment a claim transitions to `state=active`; use this tool only for manual retries or operator overrides. " +
      "Returns the dispatch payload (ops_repo, workflow filename, ref, inputs) so you can locate the resulting workflow run.",
    inputSchema: z.object({
      issue_number: z.number().int().positive(),
    }),
    path: "/tools/node.boot",
  },
  {
    name: "dd_dispatch_owner_update",
    description:
      "Dispatch the owner-update workflow to bind the assigned dd-agent's `agent_owner` to the customer's GitHub identity. " +
      "Only valid for `customer_deploy` claims (confidential agents don't register the `/owner` route). Normally fired by the orchestrator once the boot workflow has populated `agent_hostname`; expose to the LLM for manual recovery if the boot workflow callback was lost.",
    inputSchema: z.object({
      issue_number: z.number().int().positive(),
      agent_host: z.string().optional(),
    }),
    path: "/tools/dd.dispatch_owner_update",
  },
];

/**
 * Build the per-tool handler the MCP server invokes. Calls the bot,
 * forwards the JSON response as the tool's text result. The MCP host
 * shows the JSON to the LLM, which then composes a human-readable
 * answer.
 */
export function makeToolHandler(client: BotClient, tool: ToolDef) {
  return async (args: unknown) => {
    const parsed = tool.inputSchema.parse(args);
    const result = await client.call(tool.path, parsed);
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify(result, null, 2),
        },
      ],
    };
  };
}
