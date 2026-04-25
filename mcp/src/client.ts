// Thin HTTP client around the bot's /tools/* surface. Single-purpose:
// take a tool path + JSON body, return the parsed JSON response or a
// typed error the MCP layer can surface back to the LLM.
//
// Auth + base URL come from process env (validated in index.ts before
// any handler runs), so the per-call signature stays minimal.

export interface BotClient {
  call(path: string, body: unknown): Promise<unknown>;
}

export function makeBotClient(baseUrl: string, token: string): BotClient {
  // Strip trailing slashes so callers can pass either form.
  const base = baseUrl.replace(/\/+$/, "");
  return {
    async call(path: string, body: unknown): Promise<unknown> {
      const url = `${base}${path}`;
      const resp = await fetch(url, {
        method: "POST",
        headers: {
          "Authorization": `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(body),
      });
      const text = await resp.text();
      // The bot always returns JSON (incl. for errors: {"error": "..."}).
      // Parse defensively — a 5xx from a load balancer might be HTML.
      let parsed: unknown;
      try {
        parsed = text.length === 0 ? {} : JSON.parse(text);
      } catch {
        throw new Error(
          `bot ${path} → ${resp.status}: non-JSON body (${text.slice(0, 200)})`,
        );
      }
      if (!resp.ok) {
        const msg =
          (parsed as { error?: string })?.error ?? `HTTP ${resp.status}`;
        throw new Error(`bot ${path} → ${resp.status}: ${msg}`);
      }
      return parsed;
    },
  };
}
