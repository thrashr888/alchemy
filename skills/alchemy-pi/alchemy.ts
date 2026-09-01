/**
 * Alchemy bridge for pi — installed by Alchemy's Settings → Agents connect.
 *
 * pi has no MCP client by design, so this extension speaks the MCP
 * streamable-HTTP protocol directly (plain fetch, zero npm deps) to the
 * Alchemy app's embedded server and registers its core tools natively.
 * The full tool catalog stays reachable via alchemy_list_tools +
 * alchemy_call. The `__ALCHEMY_MCP_URL__` placeholder is substituted with
 * the configured server URL and private bearer token at install time.
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

const MCP_URL = "__ALCHEMY_MCP_URL__";
const MCP_TOKEN = "__ALCHEMY_MCP_TOKEN__";

let sessionId: string | null = null;
let nextId = 1;

async function post(body: unknown, session: string | null): Promise<Response> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    accept: "application/json, text/event-stream",
    authorization: `Bearer ${MCP_TOKEN}`,
  };
  if (session) headers["mcp-session-id"] = session;
  return fetch(MCP_URL, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
  });
}

/** Streamable HTTP answers either plain JSON or a one-shot SSE stream. */
async function parseRpc(res: Response, id: number): Promise<any> {
  const ct = res.headers.get("content-type") ?? "";
  if (ct.includes("text/event-stream")) {
    const text = await res.text();
    for (const line of text.split("\n")) {
      if (!line.startsWith("data:")) continue;
      try {
        const msg = JSON.parse(line.slice(5).trim());
        if (msg.id === id) return msg;
      } catch {
        // keep-alive or partial frame; skip
      }
    }
    throw new Error("no matching response in SSE stream");
  }
  return await res.json();
}

async function ensureSession(): Promise<string> {
  if (sessionId) return sessionId;
  const id = nextId++;
  const res = await post(
    {
      jsonrpc: "2.0",
      id,
      method: "initialize",
      params: {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "alchemy-pi-bridge", version: "1.0.0" },
      },
    },
    null,
  );
  if (!res.ok) {
    throw new Error(
      `Alchemy MCP initialize failed (HTTP ${res.status}) — is the Alchemy app running? (${MCP_URL})`,
    );
  }
  const sid = res.headers.get("mcp-session-id");
  const msg = await parseRpc(res, id);
  if (msg.error) throw new Error(`initialize error: ${msg.error.message}`);
  if (!sid) throw new Error("Alchemy MCP returned no session id");
  await post({ jsonrpc: "2.0", method: "notifications/initialized" }, sid);
  sessionId = sid;
  return sid;
}

async function rpc(method: string, params: unknown): Promise<any> {
  // One retry: a 404 means our session expired (the app restarted or the
  // server pruned it) — re-initialize and try again.
  for (let attempt = 0; attempt < 2; attempt++) {
    const sid = await ensureSession();
    const id = nextId++;
    const res = await post({ jsonrpc: "2.0", id, method, params }, sid);
    if (res.status === 404) {
      sessionId = null;
      continue;
    }
    if (!res.ok) throw new Error(`Alchemy MCP HTTP ${res.status}`);
    const msg = await parseRpc(res, id);
    if (msg.error) throw new Error(msg.error.message ?? "Alchemy MCP error");
    return msg.result;
  }
  throw new Error("Alchemy MCP session could not be re-established");
}

function flattenContent(result: any): string {
  const parts = (result?.content ?? []).map((c: any) =>
    c.type === "text" ? c.text : JSON.stringify(c),
  );
  const text = parts.join("\n");
  if (result?.isError) throw new Error(text || "Alchemy tool error");
  return text || JSON.stringify(result);
}

async function callTool(
  name: string,
  args: Record<string, unknown>,
): Promise<string> {
  return flattenContent(await rpc("tools/call", { name, arguments: args }));
}

function textResult(text: string) {
  return { content: [{ type: "text" as const, text }], details: {} };
}

export default function (pi: ExtensionAPI) {
  pi.registerTool({
    name: "alchemy_list_notebooks",
    label: "Alchemy: list notebooks",
    description:
      "List all Alchemy notebooks with ids, titles, timestamps, source counts, and status. Start here to find or pick a notebook.",
    parameters: Type.Object({}),
    async execute() {
      return textResult(await callTool("list_notebooks", {}));
    },
  });

  pi.registerTool({
    name: "alchemy_search",
    label: "Alchemy: search a notebook",
    description:
      "Hybrid search (vector + keyword, rank-fused) over a notebook's sources and notes. Local and cheap — call freely, several small queries beat one broad one. Returns passages with sourceId/snippet; use alchemy_get_source for a passage's full document.",
    parameters: Type.Object({
      notebook_id: Type.String({ description: "Notebook to search" }),
      query: Type.String({ description: "Natural-language query" }),
      max_results: Type.Optional(
        Type.Number({ description: "Max passages (default 6, max 20)" }),
      ),
    }),
    async execute(_id, params) {
      return textResult(await callTool("search", params as any));
    },
  });

  pi.registerTool({
    name: "alchemy_ask_everything",
    label: "Alchemy: search all notebooks",
    description:
      "Retrieve passages for a question across ALL Alchemy notebooks at once; each passage names its notebook. Use for 'which notebook has…' questions. Synthesize the answer yourself.",
    parameters: Type.Object({
      question: Type.String({ description: "The question to retrieve for" }),
    }),
    async execute(_id, params) {
      return textResult(await callTool("ask_everything", params as any));
    },
  });

  pi.registerTool({
    name: "alchemy_list_sources",
    label: "Alchemy: list sources",
    description: "List a notebook's sources (id, title, url, status, tags).",
    parameters: Type.Object({
      notebook_id: Type.String({ description: "Notebook id" }),
    }),
    async execute(_id, params) {
      return textResult(await callTool("list_sources", params as any));
    },
  });

  pi.registerTool({
    name: "alchemy_get_source",
    label: "Alchemy: read a source",
    description:
      "Read a source's full stored text by id (from alchemy_list_sources or a search passage's sourceId).",
    parameters: Type.Object({
      source_id: Type.String({ description: "Source id" }),
    }),
    async execute(_id, params) {
      return textResult(await callTool("get_source", params as any));
    },
  });

  pi.registerTool({
    name: "alchemy_add_source",
    label: "Alchemy: add a source",
    description:
      "Add a source to a notebook: a URL to fetch, or raw text. Ingestion extracts, titles, chunks, and embeds automatically. Duplicates are rejected with the existing source's title — treat that as success.",
    parameters: Type.Object({
      notebook_id: Type.String({ description: "Notebook to add to" }),
      url: Type.Optional(Type.String({ description: "Web page URL to fetch" })),
      text: Type.Optional(
        Type.String({ description: "Raw text/markdown content" }),
      ),
      title: Type.Optional(Type.String({ description: "Optional title" })),
    }),
    async execute(_id, params) {
      return textResult(await callTool("add_source", params as any));
    },
  });

  pi.registerTool({
    name: "alchemy_create_note",
    label: "Alchemy: write a note",
    description:
      "Create a markdown note in a notebook. Cite which sources informed each claim by title so the user can verify.",
    parameters: Type.Object({
      notebook_id: Type.String({ description: "Notebook to write in" }),
      title: Type.String({ description: "Note title" }),
      content: Type.String({ description: "Markdown body" }),
    }),
    async execute(_id, params) {
      return textResult(await callTool("create_note", params as any));
    },
  });

  pi.registerTool({
    name: "alchemy_list_tools",
    label: "Alchemy: list all tools",
    description:
      "List every tool the Alchemy server exposes (~46: notes, ledger, registry, schedules, Mac write-back, …) with schemas. Call anything beyond the registered alchemy_* set via alchemy_call.",
    parameters: Type.Object({}),
    async execute() {
      const result = await rpc("tools/list", {});
      const tools = (result?.tools ?? []).map((t: any) => ({
        name: t.name,
        description: t.description,
        inputSchema: t.inputSchema,
      }));
      return textResult(JSON.stringify(tools, null, 2));
    },
  });

  pi.registerTool({
    name: "alchemy_call",
    label: "Alchemy: call any tool",
    description:
      "Escape hatch: call any Alchemy tool by name (see alchemy_list_tools) with a JSON object of arguments. Never delete notebooks/notes/sources the user didn't explicitly ask to remove.",
    parameters: Type.Object({
      tool: Type.String({ description: "Tool name, e.g. get_note" }),
      arguments_json: Type.String({
        description: 'Arguments as a JSON object string, e.g. {"note_id":"…"}',
      }),
    }),
    async execute(_id, params: any) {
      let args: Record<string, unknown>;
      try {
        args = JSON.parse(params.arguments_json || "{}");
      } catch (e: any) {
        throw new Error(`arguments_json is not valid JSON: ${e.message}`);
      }
      return textResult(await callTool(params.tool, args));
    },
  });
}
