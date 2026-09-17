/**
 * Drives the Desktop Extension proxy the way Claude Desktop does: spawn it,
 * write JSON-RPC lines to its stdin, read lines back from its stdout. The
 * "app" is a throwaway HTTP server that can answer in JSON or SSE, and can
 * reject a stale token so the token re-read is exercised rather than assumed.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, test } from "node:test";

const PROXY = fileURLToPath(new URL("./index.mjs", import.meta.url));

/** A fake Alchemy: `reply(request, body)` decides each response. */
function fakeApp(reply) {
  const seen = [];
  const server = createServer((req, res) => {
    let raw = "";
    req.on("data", (chunk) => (raw += chunk));
    req.on("end", () => {
      const message = JSON.parse(raw);
      seen.push({ message, headers: req.headers });
      reply(res, message, req);
    });
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () =>
      resolve({ server, seen, port: server.address().port }),
    );
  });
}

function json(res, body, headers = {}) {
  res.writeHead(200, { "content-type": "application/json", ...headers });
  res.end(JSON.stringify(body));
}

function sse(res, messages) {
  res.writeHead(200, { "content-type": "text/event-stream" });
  res.end(messages.map((m) => `event: message\ndata: ${JSON.stringify(m)}\n\n`).join(""));
}

/** Spawn the proxy pointed at a discovery file, with a promise-based reader. */
function startProxy(discovery) {
  const child = spawn(process.execPath, [PROXY], {
    env: { ...process.env, ALCHEMY_MCP_DISCOVERY: discovery },
    stdio: ["pipe", "pipe", "pipe"],
  });
  const lines = [];
  let waiting = null;
  let buffer = "";
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    const parts = buffer.split("\n");
    buffer = parts.pop();
    for (const part of parts) {
      if (!part.trim()) continue;
      lines.push(JSON.parse(part));
      if (waiting) {
        const resolve = waiting;
        waiting = null;
        resolve(lines.shift());
      }
    }
  });
  const stderr = [];
  child.stderr.on("data", (chunk) => stderr.push(String(chunk)));
  return {
    child,
    stderr,
    send(message) {
      child.stdin.write(`${JSON.stringify(message)}\n`);
    },
    /** The next message the proxy writes to stdout. */
    next() {
      if (lines.length) return Promise.resolve(lines.shift());
      return new Promise((resolve, reject) => {
        waiting = resolve;
        setTimeout(() => reject(new Error("proxy wrote nothing")), 5000).unref();
      });
    },
    stop() {
      child.stdin.end();
      child.kill();
    },
  };
}

function discoveryFile(port, token) {
  const dir = mkdtempSync(join(tmpdir(), "alchemy-mcpb-"));
  const path = join(dir, "mcp.json");
  writeFileSync(path, JSON.stringify({ port, url: `http://127.0.0.1:${port}/mcp`, token }));
  return path;
}

const cleanup = [];
after(() => cleanup.forEach((fn) => fn()));

test("a whole session: JSON reply, SSE reply, session id, silent notification", async () => {
  const accept = "token-one";
  const { server, seen, port } = await fakeApp((res, message) => {
    if (message.method === "initialize") {
      return json(res, { jsonrpc: "2.0", id: message.id, result: { ok: true } }, {
        "mcp-session-id": "session-42",
      });
    }
    if (message.id === undefined) {
      res.writeHead(202);
      return res.end();
    }
    return sse(res, [{ jsonrpc: "2.0", id: message.id, result: { tools: [] } }]);
  });
  const proxy = startProxy(discoveryFile(port, accept));
  cleanup.push(() => {
    proxy.stop();
    server.close();
  });

  proxy.send({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} });
  assert.deepEqual(await proxy.next(), { jsonrpc: "2.0", id: 1, result: { ok: true } });
  assert.equal(seen[0].headers.authorization, `Bearer ${accept}`);
  assert.match(seen[0].headers.accept, /text\/event-stream/);

  // Notifications are forwarded and produce no output; the SSE reply that
  // follows is what proves stdout stayed clean.
  proxy.send({ jsonrpc: "2.0", method: "notifications/initialized" });
  proxy.send({ jsonrpc: "2.0", id: 2, method: "tools/list" });
  assert.deepEqual(await proxy.next(), { jsonrpc: "2.0", id: 2, result: { tools: [] } });

  assert.equal(seen.length, 3);
  assert.equal(seen[1].message.method, "notifications/initialized");
  // Everything after initialize carries the session the app minted.
  assert.equal(seen[1].headers["mcp-session-id"], "session-42");
  assert.equal(seen[2].headers["mcp-session-id"], "session-42");
});

test("a 401 makes the proxy re-read the rotated token and retry", async () => {
  let accept = "token-one";
  const { server, seen, port } = await fakeApp((res, message, req) => {
    if (req.headers.authorization !== `Bearer ${accept}`) {
      res.writeHead(401);
      return res.end();
    }
    return json(res, { jsonrpc: "2.0", id: message.id, result: { ok: true } });
  });
  const discovery = discoveryFile(port, accept);
  const proxy = startProxy(discovery);
  cleanup.push(() => {
    proxy.stop();
    server.close();
  });

  proxy.send({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} });
  await proxy.next();

  // The app relaunched: new token in the discovery file, old one refused.
  accept = "token-two";
  writeFileSync(discovery, JSON.stringify({ port, url: `http://127.0.0.1:${port}/mcp`, token: accept }));

  proxy.send({ jsonrpc: "2.0", id: 2, method: "tools/list" });
  assert.deepEqual(await proxy.next(), { jsonrpc: "2.0", id: 2, result: { ok: true } });

  const attempts = seen.filter((s) => s.message.id === 2);
  assert.equal(attempts.length, 2, "one rejected attempt, then the retry");
  assert.equal(attempts[0].headers.authorization, "Bearer token-one");
  assert.equal(attempts[1].headers.authorization, "Bearer token-two");
});

test("a refused connection answers the pending request instead of hanging", async () => {
  const { server, port } = await fakeApp((res) => res.end());
  await new Promise((resolve) => server.close(resolve));

  const proxy = startProxy(discoveryFile(port, "token-one"));
  cleanup.push(() => proxy.stop());

  proxy.send({ jsonrpc: "2.0", id: 1, method: "initialize", params: {} });
  const reply = await proxy.next();
  assert.equal(reply.id, 1);
  assert.equal(reply.error.message, "Alchemy isn't running — open it and try again");
  assert.match(proxy.stderr.join(""), /isn't running/);
});

test("a missing discovery file reads as offline, not as a crash", async () => {
  const proxy = startProxy(join(tmpdir(), "alchemy-mcpb-nothing-here.json"));
  cleanup.push(() => proxy.stop());

  proxy.send({ jsonrpc: "2.0", id: 7, method: "tools/list" });
  const reply = await proxy.next();
  assert.equal(reply.id, 7);
  assert.equal(reply.error.code, -32001);
});
