#!/usr/bin/env node
//
// @satsforcompute/mcp — MCP server for the Sats for Compute bot.
//
// Stdio transport. Reads SATS_BOT_URL + SATS_TOOL_API_TOKEN from env
// and exposes the bot's six /tools/* endpoints as MCP tools.

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { zodToJsonSchema } from "zod-to-json-schema";

import { makeBotClient } from "./client.js";
import { TOOLS, makeToolHandler } from "./tools.js";

function requireEnv(key: string): string {
  const v = process.env[key];
  if (!v || v.length === 0) {
    process.stderr.write(
      `[satsforcompute-mcp] ${key} must be set; refusing to start.\n` +
        `  See https://github.com/satsforcompute/satsforcompute/tree/main/mcp#config\n`,
    );
    process.exit(1);
  }
  return v;
}

async function main() {
  const botUrl = requireEnv("SATS_BOT_URL");
  const token = requireEnv("SATS_TOOL_API_TOKEN");
  const client = makeBotClient(botUrl, token);

  const server = new Server(
    {
      name: "satsforcompute",
      version: "0.1.0",
    },
    {
      capabilities: { tools: {} },
    },
  );

  // Map each ToolDef to (a) the list response and (b) a dispatch entry.
  const handlers = new Map(
    TOOLS.map((t) => [t.name, makeToolHandler(client, t)] as const),
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: TOOLS.map((t) => ({
      name: t.name,
      description: t.description,
      inputSchema: zodToJsonSchema(t.inputSchema, { target: "openApi3" }),
    })),
  }));

  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    const handler = handlers.get(req.params.name);
    if (!handler) {
      throw new Error(`unknown tool: ${req.params.name}`);
    }
    return handler(req.params.arguments ?? {});
  });

  const transport = new StdioServerTransport();
  await server.connect(transport);
  process.stderr.write(
    `[satsforcompute-mcp] connected (bot=${botUrl}, ${TOOLS.length} tools)\n`,
  );
}

main().catch((err) => {
  process.stderr.write(`[satsforcompute-mcp] fatal: ${err}\n`);
  process.exit(1);
});
