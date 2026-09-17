#!/usr/bin/env node

/**
 * Alchemy's Desktop Extension server: stdio JSON-RPC in, streamable HTTP out.
 *
 * Claude Desktop runs an extension as a stdio child under its own bundled
 * Node, so nothing has to be installed on the Mac. Alchemy's MCP server is
 * streamable HTTP behind a private bearer token on 127.0.0.1, so this script
 * is the whole bridge between the two:
 *
 *   1. read the app's discovery file (url + token) — never baked in here,
 *      because the app rewrites it every launch;
 *   2. read newline-delimited JSON-RPC from stdin;
 *   3. POST each message to the endpoint with the token, carrying the
 *      `mcp-session-id` the `initialize` reply hands back;
 *   4. write every JSON-RPC message that comes back — plain JSON, or the
 *      `data:` lines of an SSE body — to stdout, one per line.
 *
 * Node 18+ standard library only: no dependencies to install, vendor, or
 * audit inside a bundle the app writes on the user's own machine.
 */

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

/** Where the running app advertises its port and private token. */
const DISCOVERY =
  process.env.ALCHEMY_MCP_DISCOVERY ||
  join(homedir(), "Library", "Application Support", "com.thrashr888.alchemy", "mcp.json");

/** The one failure worth explaining in the conversation rather than the log. */
const OFFLINE = "Alchemy isn't running — open it and try again";

class Offline extends Error {}

/** Cached connection. Dropped whenever the app may have moved: no file, a
 *  refused socket, or a token the server no longer accepts. */
let connection = null;
/** Set from the `initialize` reply; sent on everything after it. */
let sessionId = null;

/** Read url + token fresh. The token rotates when the app relaunches, so the
 *  file — not this process's memory — is the source of truth. */
function connect() {
  if (connection) return connection;
  let info;
  try {
    info = JSON.parse(readFileSync(DISCOVERY, "utf8"));
  } catch {
    throw new Offline();
  }
  const url =
    typeof info.url === "string"
      ? info.url
      : Number.isInteger(info.port)
        ? `http://127.0.0.1:${info.port}/mcp`
        : null;
  if (!url) throw new Offline();
  connection = { url, token: typeof info.token === "string" ? info.token : "" };
  return connection;
}

async function send(message) {
  const { url, token } = connect();
  const headers = {
    "content-type": "application/json",
    accept: "application/json, text/event-stream",
  };
  if (token) headers.authorization = `Bearer ${token}`;
  if (sessionId) headers["mcp-session-id"] = sessionId;
  try {
    return await fetch(url, { method: "POST", headers, body: JSON.stringify(message) });
  } catch {
    // Connection refused: the app quit, or listens somewhere else now.
    connection = null;
    throw new Offline();
  }
}

/** A reply is either one JSON-RPC message or an SSE body whose `data:` lines
 *  each carry one. A notification gets 202 and no body at all. */
async function decode(response) {
  if (response.status === 202 || response.status === 204) return [];
  const body = await response.text();
  if (!body.trim()) return [];
  const type = response.headers.get("content-type") || "";
  if (!type.includes("text/event-stream")) return [JSON.parse(body)];

  const messages = [];
  let data = [];
  const flush = () => {
    if (!data.length) return;
    try {
      messages.push(JSON.parse(data.join("\n")));
    } catch {
      // Comments and keep-alives carry no JSON.
    }
    data = [];
  };
  for (const line of body.split(/\r?\n/)) {
    if (line === "") flush();
    else if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
  }
  flush();
  return messages;
}

/** stdout belongs to the protocol; everything human goes to stderr. */
function emit(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function log(text) {
  process.stderr.write(`alchemy: ${text}\n`);
}

async function handle(line) {
  const text = line.trim();
  if (!text) return;
  let message;
  try {
    message = JSON.parse(text);
  } catch {
    log(`ignoring a line that isn't JSON: ${text.slice(0, 120)}`);
    return;
  }

  try {
    let response = await send(message);
    // A relaunch mints a new session and can rotate the token. Re-read the
    // discovery file once and retry, so that costs a round trip instead of
    // the rest of the conversation.
    if (response.status === 401) {
      connection = null;
      sessionId = null;
      response = await send(message);
    }
    if (!response.ok) throw new Error(`Alchemy replied HTTP ${response.status}`);
    const id = response.headers.get("mcp-session-id");
    if (id) sessionId = id;
    for (const reply of await decode(response)) emit(reply);
  } catch (error) {
    const reason = error instanceof Offline ? OFFLINE : String(error?.message || error);
    log(reason);
    // Notifications have no id and get no reply — answering one would itself
    // be a protocol error. A request must never be left hanging.
    if (message.id === undefined || message.id === null) return;
    emit({ jsonrpc: "2.0", id: message.id, error: { code: -32001, message: reason } });
  }
}

const input = createInterface({ input: process.stdin });
let handshake = null;
input.on("line", (line) => {
  // `initialize` has to land before anything else goes out: its reply carries
  // the session id the rest of the conversation needs. After that, messages
  // are independent — a slow search must not hold up a cancellation.
  if (!handshake) handshake = handle(line);
  else void handshake.then(() => handle(line));
});
input.on("close", () => process.exit(0));
