import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  CliError,
  McpClient,
  discoverMcpConnection,
  discoverMcpUrl,
  parseArgs,
  resolveNotebook,
  sourceArguments,
  visibleNotebooks,
} from "./alchemy.mjs";

test("archived notebooks are hidden unless --all", () => {
  const list = [
    { id: "nb-1", title: "Active" },
    { id: "nb-2", title: "Old", status: "archived" },
    { id: "nb-3", title: "Briefs", status: "system" },
  ];
  assert.deepEqual(
    visibleNotebooks(list, false).map((n) => n.id),
    ["nb-1", "nb-3"],
  );
  assert.deepEqual(
    visibleNotebooks(list, true).map((n) => n.id),
    ["nb-1", "nb-2", "nb-3"],
  );
});

test("parses the narrow command surface", () => {
  assert.deepEqual(parseArgs(["notebooks", "--json"]), {
    command: "notebooks",
    mcpUrl: undefined,
    mcpToken: undefined,
    json: true,
    all: false,
  });
  assert.equal(parseArgs(["notebooks", "--all"]).all, true);
  assert.equal(parseArgs(["-h"]).command, "help");
  assert.equal(parseArgs(["-v"]).command, "version");
  assert.equal(parseArgs(["--version"]).command, "version");
  assert.deepEqual(
    parseArgs(["search", "renewal", "risk", "--notebook", "Atlas", "--limit", "4"]),
    {
      command: "search",
      mcpUrl: undefined,
      mcpToken: undefined,
      json: false,
      notebook: "Atlas",
      query: "renewal risk",
      limit: 4,
    },
  );
  assert.throws(() => parseArgs(["search", "query", "--limit", "21"]), CliError);
  assert.throws(() => parseArgs(["add", "a", "b", "--notebook", "Atlas", "--title", "x"]), CliError);
  assert.throws(() => parseArgs(["add", "a.pdf", "--notebook", "Atlas", "--title", "x"]), CliError);
});

test("discovers the running app and honors explicit overrides", async () => {
  const dir = await mkdtemp(join(tmpdir(), "alchemy-cli-"));
  const discovery = join(dir, "mcp.json");
  await writeFile(discovery, JSON.stringify({ port: 43123, token: "secret" }));
  assert.equal(
    await discoverMcpUrl(undefined, { ALCHEMY_MCP_DISCOVERY: discovery }),
    "http://127.0.0.1:43123/mcp",
  );
  assert.deepEqual(
    await discoverMcpConnection(undefined, undefined, { ALCHEMY_MCP_DISCOVERY: discovery }),
    { url: "http://127.0.0.1:43123/mcp", token: "secret" },
  );
  assert.equal(
    await discoverMcpUrl("http://localhost:9999/mcp", {}),
    "http://localhost:9999/mcp",
  );
});

test("a tokenless discovery file (older app) still connects", async () => {
  const dir = await mkdtemp(join(tmpdir(), "alchemy-cli-old-"));
  const discovery = join(dir, "mcp.json");
  await writeFile(discovery, JSON.stringify({ port: 41414 }));
  assert.deepEqual(
    await discoverMcpConnection(undefined, undefined, { ALCHEMY_MCP_DISCOVERY: discovery }),
    { url: "http://127.0.0.1:41414/mcp", token: "" },
  );
});

test("classifies URLs, files, and stdin for add_source", async () => {
  const dir = await mkdtemp(join(tmpdir(), "alchemy-cli-source-"));
  const file = join(dir, "notes.md");
  await writeFile(file, "hello");
  assert.deepEqual(await sourceArguments("https://example.com", undefined), {
    url: "https://example.com",
  });
  assert.deepEqual(await sourceArguments(file, undefined), { file_path: file });
  assert.deepEqual(await sourceArguments("-", "Standup", "hello\n"), {
    text: "hello\n",
    title: "Standup",
  });
  await assert.rejects(() => sourceArguments("-", undefined, "  "), /stdin was empty/);
});

test("resolves notebooks only by exact id or exact case-insensitive title", async () => {
  const client = {
    call: async () => [
      { id: "nb-1", title: "Project Atlas" },
      { id: "nb-2", title: "Other" },
    ],
  };
  assert.equal((await resolveNotebook(client, "nb-1")).title, "Project Atlas");
  assert.equal((await resolveNotebook(client, "project atlas")).id, "nb-1");
  await assert.rejects(() => resolveNotebook(client, "Atlas"), /no notebook/);
});

test("MCP client authenticates, initializes a session, and parses a JSON tool result", async () => {
  const requests = [];
  const fetchImpl = async (_url, init) => {
    const request = JSON.parse(init.body);
    requests.push({ request, headers: init.headers });
    if (request.method === "initialize") {
      return new Response(
        JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { protocolVersion: "2025-06-18" } }),
        { headers: { "content-type": "application/json", "mcp-session-id": "session-1" } },
      );
    }
    if (request.method === "notifications/initialized") return new Response(null, { status: 202 });
    return new Response(
      JSON.stringify({
        jsonrpc: "2.0",
        id: request.id,
        result: { content: [{ type: "text", text: '[{"id":"nb-1","title":"Atlas"}]' }] },
      }),
      { headers: { "content-type": "application/json" } },
    );
  };
  const client = new McpClient("http://127.0.0.1:41414/mcp", "local-secret", fetchImpl);
  assert.deepEqual(await client.call("list_notebooks"), [{ id: "nb-1", title: "Atlas" }]);
  assert.equal(requests[0].headers.authorization, "Bearer local-secret");
  assert.equal(requests[2].headers["mcp-session-id"], "session-1");
});

test("MCP client ignores SSE progress and returns the matching response", async () => {
  let initialized = false;
  const fetchImpl = async (_url, init) => {
    const request = JSON.parse(init.body);
    if (request.method === "initialize") {
      return new Response(
        `event: message\ndata: {"jsonrpc":"2.0","id":${request.id},"result":{}}\n\n`,
        { headers: { "content-type": "text/event-stream", "mcp-session-id": "sse-1" } },
      );
    }
    if (request.method === "notifications/initialized") {
      initialized = true;
      return new Response(null, { status: 202 });
    }
    return new Response(
      `event: message\ndata: {"jsonrpc":"2.0","method":"notifications/progress","params":{}}\n\n` +
        `event: message\ndata: {"jsonrpc":"2.0","id":${request.id},"result":{"content":[{"type":"text","text":"[]"}]}}\n\n`,
      { headers: { "content-type": "text/event-stream" } },
    );
  };
  const client = new McpClient("http://127.0.0.1:41414/mcp", fetchImpl);
  assert.deepEqual(await client.call("search", {}), []);
  assert.equal(initialized, true);
});
