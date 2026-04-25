import { afterEach, describe, expect, it, vi } from "vitest";
import { makeBotClient } from "../src/client.js";
import { TOOLS, makeToolHandler } from "../src/tools.js";

const BASE = "https://bot.example/";
const TOKEN = "test-token";

function mockFetchOnce(status: number, body: unknown) {
  const fetchMock = vi
    .spyOn(globalThis, "fetch")
    .mockResolvedValueOnce(
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
    );
  return fetchMock;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("BotClient", () => {
  it("posts JSON with bearer auth and returns parsed body on 200", async () => {
    const fetchMock = mockFetchOnce(200, { ok: true });
    const client = makeBotClient(BASE, TOKEN);
    const out = await client.call("/tools/claim.create", { mode: "x" });
    expect(out).toEqual({ ok: true });

    const [url, init] = fetchMock.mock.calls[0]!;
    // Trailing slash on BASE is normalized; tool path is appended.
    expect(url).toBe("https://bot.example/tools/claim.create");
    expect(init?.method).toBe("POST");
    const headers = init?.headers as Record<string, string>;
    expect(headers.Authorization).toBe(`Bearer ${TOKEN}`);
    expect(headers["Content-Type"]).toBe("application/json");
    expect(init?.body).toBe(JSON.stringify({ mode: "x" }));
  });

  it("surfaces the bot's error message on non-2xx", async () => {
    mockFetchOnce(401, { error: "unauthorized" });
    const client = makeBotClient(BASE, TOKEN);
    await expect(client.call("/tools/x", {})).rejects.toThrow(/401.*unauthorized/);
  });

  it("throws cleanly when the response body isn't JSON", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(
      new Response("<html>edge timeout</html>", { status: 504 }),
    );
    const client = makeBotClient(BASE, TOKEN);
    await expect(client.call("/tools/x", {})).rejects.toThrow(/non-JSON/);
  });
});

describe("tool registry", () => {
  it("declares one tool per /tools/ endpoint with snake_case names", () => {
    const expected = new Set([
      "claim_create",
      "claim_load",
      "claim_update",
      "btc_invoice",
      "node_boot",
      "dd_dispatch_owner_update",
    ]);
    const actual = new Set(TOOLS.map((t) => t.name));
    expect(actual).toEqual(expected);
  });

  it("each tool path matches /tools/<dotted-name>", () => {
    for (const tool of TOOLS) {
      expect(tool.path).toMatch(/^\/tools\/[a-z][a-z._]+$/);
    }
  });
});

describe("makeToolHandler", () => {
  it("validates input via zod and dispatches to the bot", async () => {
    const claimCreate = TOOLS.find((t) => t.name === "claim_create")!;
    mockFetchOnce(200, { claim: {}, issue_number: 7, issue_url: "..." });
    const client = makeBotClient(BASE, TOKEN);
    const handler = makeToolHandler(client, claimCreate);

    const result = await handler({
      mode: "customer_deploy",
      customer_owner: "alice",
    });
    // MCP tool result: a content array with one text block carrying the
    // JSON the bot returned, ready for the LLM to render.
    expect(result.content).toHaveLength(1);
    expect(result.content[0]!.type).toBe("text");
    const echoed = JSON.parse(result.content[0]!.text);
    expect(echoed.issue_number).toBe(7);
  });

  it("rejects malformed input before touching the network", async () => {
    const claimCreate = TOOLS.find((t) => t.name === "claim_create")!;
    const fetchMock = vi.spyOn(globalThis, "fetch");
    const client = makeBotClient(BASE, TOKEN);
    const handler = makeToolHandler(client, claimCreate);

    // mode must be one of the two enum values; "bogus" fails zod.
    await expect(handler({ mode: "bogus" })).rejects.toThrow();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("confidential claim with workload_repo passes validation", async () => {
    const claimCreate = TOOLS.find((t) => t.name === "claim_create")!;
    mockFetchOnce(200, { claim: {}, issue_number: 8, issue_url: "..." });
    const client = makeBotClient(BASE, TOKEN);
    const handler = makeToolHandler(client, claimCreate);

    const result = await handler({
      mode: "confidential",
      workload_repo: "alice/oracle",
      workload_ref: "v1.0",
    });
    expect(JSON.parse(result.content[0]!.text).issue_number).toBe(8);
  });
});
