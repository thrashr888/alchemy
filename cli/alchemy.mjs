#!/usr/bin/env node

/**
 * Thin command-line client for the MCP server embedded in the running app.
 *
 * Intentionally no data access or headless app bootstrap lives here: the
 * Tauri process remains the one owner of Alchemy's database and models.
 */

import { realpathSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const { version: VERSION } = createRequire(import.meta.url)("./package.json");
const DEFAULT_MCP_URL = "http://127.0.0.1:41414/mcp";

const HELP = `Alchemy CLI — add to and search the running Alchemy app

Usage:
  alchemy notebooks [--all] [--json]
  alchemy add <file-or-url>... --notebook <id-or-title> [--title <title>] [--json]
  alchemy add - --notebook <id-or-title> [--title <title>] [--json]
  alchemy search <query...> [--notebook <id-or-title>] [--limit <1-20>] [--json]
  alchemy events [--notebook <id-or-title>] [--kinds <k,k>] [--since <ms|24h|7d>] [--follow] [--json]

Commands:
  notebooks  List notebooks and their ids (--all includes archived).
  add        Add local files, web URLs, or stdin (use - or pipe with no input).
  search     Search one notebook, or all notebooks when --notebook is omitted.
  events     Source-change events (added, updated, removed, unreachable,
             completed, moved). --follow tails the app's live stream.

Connection:
  The Alchemy app must be running with MCP enabled. The CLI discovers the
  app and its private local token through mcp.json. Override with --mcp-url
  and --mcp-token, or ALCHEMY_MCP_URL and ALCHEMY_MCP_TOKEN.

Examples:
  alchemy notebooks
  alchemy add report.pdf https://example.com --notebook "Project Atlas"
  pbpaste | alchemy add --notebook "Project Atlas" --title "Meeting notes"
  alchemy search "renewal risk" --notebook "Project Atlas"
  alchemy search "where did I save the contractor agreement?" --json
  alchemy events --kinds added --since 7d
  alchemy events --follow --json | jq -c 'select(.kind == "added")'`;

export const EVENT_KINDS = ["added", "updated", "removed", "unreachable", "completed", "moved"];

/** "24h" / "7d" / "90m" / a raw epoch-millisecond number → epoch ms floor. */
export function parseSince(value, now = Date.now()) {
  const m = /^(\d+)([mhd])$/.exec(value);
  if (m) {
    const unit = { m: 60_000, h: 3_600_000, d: 86_400_000 }[m[2]];
    return now - Number(m[1]) * unit;
  }
  if (/^\d{10,}$/.test(value)) return Number(value);
  throw new CliError("--since takes a duration like 24h, 7d, 90m, or an epoch-millisecond timestamp");
}

/** The live stream sits beside the MCP endpoint: …/mcp → …/events. */
export function eventsUrl(mcpUrl) {
  const url = new URL(mcpUrl);
  url.pathname = url.pathname.replace(/\/mcp\/?$/, "/events");
  if (!url.pathname.endsWith("/events")) url.pathname = "/events";
  url.search = "";
  return url.toString();
}

/** Incremental Server-Sent Events decoder: feed it chunks, get back the
 *  parsed `data:` payloads of every complete event, keeping the tail. */
export function sseEvents(buffer, chunk) {
  const text = buffer + chunk;
  const events = [];
  const blocks = text.split(/\r?\n\r?\n/);
  const rest = blocks.pop();
  for (const block of blocks) {
    const data = block
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart())
      .join("\n");
    if (!data) continue;
    try {
      events.push(JSON.parse(data));
    } catch {
      /* comments and keep-alives carry no JSON */
    }
  }
  return { events, rest };
}

export function formatEvent(e) {
  const at = new Date(e.at);
  const stamp = `${at.getHours().toString().padStart(2, "0")}:${at.getMinutes().toString().padStart(2, "0")}`;
  return `${stamp}  ${e.kind.padEnd(11)} ${e.sourceTitle} — ${e.detail}`;
}

export class CliError extends Error {}

function takeOption(args, longName) {
  const at = args.indexOf(longName);
  if (at === -1) return undefined;
  if (at === args.length - 1 || args[at + 1].startsWith("--")) {
    throw new CliError(`${longName} needs a value`);
  }
  const value = args[at + 1];
  args.splice(at, 2);
  if (args.includes(longName)) throw new CliError(`${longName} may only be used once`);
  return value;
}

function takeFlag(args, longName) {
  const found = args.includes(longName);
  if (!found) return false;
  args.splice(args.indexOf(longName), 1);
  if (args.includes(longName)) throw new CliError(`${longName} may only be used once`);
  return true;
}

export function parseArgs(argv) {
  const args = [...argv];
  if (
    args.length === 0 ||
    args[0] === "help" ||
    takeFlag(args, "--help") ||
    takeFlag(args, "-h")
  ) {
    return { command: "help" };
  }
  if (takeFlag(args, "--version") || takeFlag(args, "-v")) {
    return { command: "version" };
  }

  const command = args.shift();
  const mcpUrl = takeOption(args, "--mcp-url");
  const mcpToken = takeOption(args, "--mcp-token");
  const json = takeFlag(args, "--json");

  if (command === "notebooks") {
    const all = takeFlag(args, "--all");
    if (args.length) throw new CliError(`unexpected argument: ${args[0]}`);
    return { command, mcpUrl, mcpToken, json, all };
  }

  if (command === "add") {
    const notebook = takeOption(args, "--notebook");
    const title = takeOption(args, "--title");
    if (!notebook) throw new CliError("add requires --notebook <id-or-title>");
    if (args.some((arg) => arg.startsWith("--"))) {
      throw new CliError(`unknown option: ${args.find((arg) => arg.startsWith("--"))}`);
    }
    if (args.filter((arg) => arg === "-").length > 1) {
      throw new CliError("stdin (-) may only be added once");
    }
    if (title && (args.length > 1 || (args.length === 1 && args[0] !== "-"))) {
      throw new CliError("--title can only be used when adding one stdin source");
    }
    return { command, mcpUrl, mcpToken, json, notebook, title, inputs: args };
  }

  if (command === "search") {
    const notebook = takeOption(args, "--notebook");
    const rawLimit = takeOption(args, "--limit");
    if (args.some((arg) => arg.startsWith("--"))) {
      throw new CliError(`unknown option: ${args.find((arg) => arg.startsWith("--"))}`);
    }
    const query = args.join(" ").trim();
    if (!query) throw new CliError("search needs a query");
    let limit;
    if (rawLimit !== undefined) {
      limit = Number(rawLimit);
      if (!Number.isInteger(limit) || limit < 1 || limit > 20) {
        throw new CliError("--limit must be an integer from 1 to 20");
      }
    }
    return { command, mcpUrl, mcpToken, json, notebook, query, limit };
  }

  if (command === "events") {
    const notebook = takeOption(args, "--notebook");
    const rawKinds = takeOption(args, "--kinds");
    const rawSince = takeOption(args, "--since");
    const follow = takeFlag(args, "--follow");
    if (args.length) throw new CliError(`unexpected argument: ${args[0]}`);
    const kinds = rawKinds
      ? rawKinds
          .split(",")
          .map((k) => k.trim())
          .filter(Boolean)
      : undefined;
    const bad = kinds?.find((k) => !EVENT_KINDS.includes(k));
    if (bad) throw new CliError(`unknown event kind: ${bad} (one of ${EVENT_KINDS.join(", ")})`);
    return { command, mcpUrl, mcpToken, json, notebook, kinds, since: rawSince, follow };
  }

  throw new CliError(`unknown command: ${command}`);
}

export function discoveryPaths(env = process.env, home = homedir(), platform = process.platform) {
  if (env.ALCHEMY_MCP_DISCOVERY) return [env.ALCHEMY_MCP_DISCOVERY];
  if (platform === "darwin") {
    return [`${home}/Library/Application Support/com.thrashr888.alchemy/mcp.json`];
  }
  if (platform === "win32") {
    return env.APPDATA ? [`${env.APPDATA}/com.thrashr888.alchemy/mcp.json`] : [];
  }
  return [
    `${env.XDG_DATA_HOME || `${home}/.local/share`}/com.thrashr888.alchemy/mcp.json`,
  ];
}

export async function discoverMcpUrl(explicit, env = process.env) {
  return (await discoverMcpConnection(explicit, undefined, env)).url;
}

export async function discoverMcpConnection(explicitUrl, explicitToken, env = process.env) {
  const overrideUrl = explicitUrl || env.ALCHEMY_MCP_URL;
  const overrideToken = explicitToken || env.ALCHEMY_MCP_TOKEN || "";
  if (overrideUrl) {
    return {
      url: validateMcpUrl(overrideUrl, explicitUrl ? "--mcp-url" : "ALCHEMY_MCP_URL"),
      token: overrideToken,
    };
  }

  for (const path of discoveryPaths(env)) {
    try {
      const info = JSON.parse(await readFile(path, "utf8"));
      const url =
        typeof info.url === "string"
          ? validateMcpUrl(info.url, path)
          : Number.isInteger(info.port)
            ? `http://127.0.0.1:${info.port}/mcp`
            : null;
      if (!url) throw new CliError(`Alchemy discovery file has no url or port: ${path}`);
      // An app from before local authentication writes no token, and its
      // server accepts tokenless requests — proceed. Against a newer app
      // the 401 hint explains the mismatch.
      const token = typeof info.token === "string" ? info.token : "";
      return { url, token: overrideToken || token };
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      if (error instanceof CliError) throw error;
      throw new CliError(`could not read Alchemy discovery file ${path}: ${error.message}`);
    }
  }
  return { url: DEFAULT_MCP_URL, token: overrideToken };
}

function validateMcpUrl(value, source) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new CliError(`${source} is not a valid URL: ${value}`);
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new CliError(`${source} must use http or https`);
  }
  return url.toString();
}

async function parseRpcResponse(response, id) {
  const contentType = response.headers.get("content-type") || "";
  if (!contentType.includes("text/event-stream")) return response.json();

  const messages = [];
  let data = [];
  for (const line of (await response.text()).split(/\r?\n/)) {
    if (line === "") {
      if (data.length) {
        try {
          messages.push(JSON.parse(data.join("\n")));
        } catch {
          // Ignore keep-alives and non-JSON events.
        }
        data = [];
      }
    } else if (line.startsWith("data:")) {
      data.push(line.slice(5).trimStart());
    }
  }
  if (data.length) {
    try {
      messages.push(JSON.parse(data.join("\n")));
    } catch {
      // The useful error below is clearer than a JSON parser error.
    }
  }
  const message = messages.find((candidate) => candidate.id === id);
  if (!message) throw new CliError("Alchemy returned no matching MCP response");
  return message;
}

export class McpClient {
  constructor(url, tokenOrFetch = "", fetchImpl = globalThis.fetch) {
    this.url = url;
    // Preserve the pre-token constructor shape for downstream imports while
    // making authenticated use the normal path for the bundled CLI.
    this.token = typeof tokenOrFetch === "string" ? tokenOrFetch : "";
    this.fetch = typeof tokenOrFetch === "function" ? tokenOrFetch : fetchImpl;
    this.sessionId = null;
    this.nextId = 1;
  }

  async post(body, sessionId = this.sessionId) {
    const headers = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
    };
    if (this.token) headers.authorization = `Bearer ${this.token}`;
    if (sessionId) headers["mcp-session-id"] = sessionId;
    try {
      return await this.fetch(this.url, {
        method: "POST",
        headers,
        body: JSON.stringify(body),
      });
    } catch (error) {
      throw new CliError(
        `could not reach Alchemy at ${this.url}; is the app running with MCP enabled? (${error.message})`,
      );
    }
  }

  async initialize() {
    if (this.sessionId) return;
    const id = this.nextId++;
    const response = await this.post(
      {
        jsonrpc: "2.0",
        id,
        method: "initialize",
        params: {
          protocolVersion: "2025-06-18",
          capabilities: {},
          clientInfo: { name: "alchemy-cli", version: VERSION },
        },
      },
      null,
    );
    if (!response.ok) {
      const hint =
        response.status === 401
          ? "; authentication failed — restart Alchemy or remove connection overrides to rediscover it"
          : "";
      throw new CliError(`Alchemy MCP initialize failed (HTTP ${response.status}${hint})`);
    }
    const message = await parseRpcResponse(response, id);
    if (message.error) throw new CliError(message.error.message || "Alchemy MCP initialize failed");
    this.sessionId = response.headers.get("mcp-session-id");
    if (!this.sessionId) throw new CliError("Alchemy MCP did not create a session");
    const notification = await this.post({ jsonrpc: "2.0", method: "notifications/initialized" });
    if (!notification.ok) {
      throw new CliError(`Alchemy MCP session setup failed (HTTP ${notification.status})`);
    }
  }

  async call(tool, argumentsObject = {}) {
    await this.initialize();
    const id = this.nextId++;
    const response = await this.post({
      jsonrpc: "2.0",
      id,
      method: "tools/call",
      params: { name: tool, arguments: argumentsObject },
    });
    if (!response.ok) throw new CliError(`Alchemy MCP request failed (HTTP ${response.status})`);
    const message = await parseRpcResponse(response, id);
    if (message.error) throw new CliError(message.error.message || `${tool} failed`);
    const result = message.result;
    const text = (result?.content || [])
      .filter((part) => part.type === "text")
      .map((part) => part.text)
      .join("\n");
    if (result?.isError) throw new CliError(text || `${tool} failed`);
    if (!text) return result;
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }
}

export async function resolveNotebook(client, requested) {
  const notebooks = await client.call("list_notebooks");
  if (!Array.isArray(notebooks)) throw new CliError("Alchemy returned an invalid notebook list");
  const byId = notebooks.find((notebook) => notebook.id === requested);
  if (byId) return byId;
  const byTitle = notebooks.filter(
    (notebook) => notebook.title?.toLocaleLowerCase() === requested.toLocaleLowerCase(),
  );
  if (byTitle.length === 1) return byTitle[0];
  if (byTitle.length > 1) {
    throw new CliError(`more than one notebook is titled "${requested}"; use its id instead`);
  }
  throw new CliError(`no notebook has id or exact title "${requested}"; run 'alchemy notebooks'`);
}

function isSupportedUrl(input) {
  try {
    return ["http:", "https:", "cider:"].includes(new URL(input).protocol);
  } catch {
    return false;
  }
}

export async function sourceArguments(input, title, stdinText) {
  if (input === "-") {
    if (!stdinText?.trim()) throw new CliError("stdin was empty");
    return { text: stdinText, ...(title ? { title } : {}) };
  }
  if (isSupportedUrl(input)) return { url: input };
  const path = resolve(input);
  let metadata;
  try {
    metadata = await stat(path);
  } catch (error) {
    if (error?.code === "ENOENT") throw new CliError(`file not found: ${input}`);
    throw error;
  }
  if (!metadata.isFile()) throw new CliError(`not a file: ${input}`);
  return { file_path: path };
}

/** Archived notebooks are shelved work — surfaced only on request. */
export function visibleNotebooks(notebooks, all) {
  return all ? notebooks : notebooks.filter((notebook) => notebook.status !== "archived");
}

function printNotebooks(notebooks) {
  if (!notebooks.length) {
    console.log("No notebooks.");
    return;
  }
  for (const notebook of notebooks) {
    const status = notebook.status ? ` [${notebook.status}]` : "";
    console.log(`${notebook.id}\t${notebook.title}\t${notebook.sourceCount ?? 0} sources${status}`);
  }
}

function printAdded(sources) {
  for (const source of sources) {
    console.log(`Added ${source.title || "source"}${source.id ? ` (${source.id})` : ""}`);
  }
}

function printSearch(results) {
  if (!Array.isArray(results) || !results.length) {
    console.log("No matches.");
    return;
  }
  for (const [index, result] of results.entries()) {
    const notebook = result.notebookTitle ? ` · ${result.notebookTitle}` : "";
    const title = result.sourceTitle || result.title || "Untitled";
    const snippet = result.snippet || result.text || "";
    console.log(`${index + 1}. ${title}${notebook}`);
    console.log(`   ${String(snippet).replace(/\s+/g, " ").trim()}`);
    const id = result.sourceId || result.noteId;
    if (id) console.log(`   ${id}`);
  }
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}

export async function run(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.command === "help") {
    console.log(HELP);
    return;
  }
  if (options.command === "version") {
    console.log(VERSION);
    return;
  }

  const connection = await discoverMcpConnection(options.mcpUrl, options.mcpToken);
  const client = new McpClient(connection.url, connection.token);
  if (options.command === "notebooks") {
    const notebooks = visibleNotebooks(await client.call("list_notebooks"), options.all);
    options.json ? console.log(JSON.stringify(notebooks, null, 2)) : printNotebooks(notebooks);
    return;
  }

  if (options.command === "add") {
    const notebook = await resolveNotebook(client, options.notebook);
    const inputs = [...options.inputs];
    if (!inputs.length) {
      if (process.stdin.isTTY) throw new CliError("add needs a file, URL, or piped stdin");
      inputs.push("-");
    }
    const needsStdin = inputs.includes("-");
    const stdinText = needsStdin ? await readStdin() : undefined;
    const sources = [];
    for (const input of inputs) {
      const args = await sourceArguments(input, options.title, stdinText);
      sources.push(await client.call("add_source", { notebook_id: notebook.id, ...args }));
    }
    options.json ? console.log(JSON.stringify(sources, null, 2)) : printAdded(sources);
    return;
  }

  if (options.command === "events") {
    const notebook = options.notebook ? await resolveNotebook(client, options.notebook) : null;
    const since = options.since ? parseSince(options.since) : undefined;
    const filter = {
      ...(notebook ? { notebook_id: notebook.id } : {}),
      ...(options.kinds ? { kinds: options.kinds } : {}),
    };
    const events = await client.call("list_source_events", {
      ...filter,
      ...(since !== undefined ? { since } : {}),
    });
    const emit = (e) => console.log(options.json ? JSON.stringify(e) : formatEvent(e));
    // Newest first from the tool; a log reads oldest first.
    let newest = since ?? 0;
    for (const e of [...events].reverse()) {
      emit(e);
      newest = Math.max(newest, e.at);
    }
    if (!options.follow) return;
    // The live stream replays from the newest event already printed, so
    // nothing lands twice and nothing between the read and the connect is
    // lost. Filters apply client-side; the stream carries everything.
    const url = new URL(eventsUrl(connection.url));
    url.searchParams.set("since", String(newest));
    const headers = { accept: "text/event-stream" };
    if (connection.token) headers.authorization = `Bearer ${connection.token}`;
    let response;
    try {
      response = await fetch(url, { headers });
    } catch (error) {
      throw new CliError(`could not open the event stream at ${url} (${error.message})`);
    }
    if (!response.ok || !response.body) {
      throw new CliError(`event stream refused: HTTP ${response.status} (this app may predate /events)`);
    }
    let buffer = "";
    for await (const chunk of response.body.pipeThrough(new TextDecoderStream())) {
      const parsed = sseEvents(buffer, chunk);
      buffer = parsed.rest;
      for (const e of parsed.events) {
        if (notebook && e.notebookId !== notebook.id) continue;
        if (options.kinds && !options.kinds.includes(e.kind)) continue;
        emit(e);
      }
    }
    return;
  }

  const results = options.notebook
    ? await (async () => {
        const notebook = await resolveNotebook(client, options.notebook);
        return client.call("search", {
          notebook_id: notebook.id,
          query: options.query,
          ...(options.limit ? { max_results: options.limit } : {}),
        });
      })()
    : await client.call("ask_everything", { question: options.query });
  options.json ? console.log(JSON.stringify(results, null, 2)) : printSearch(results);
}

const isEntrypoint =
  process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url);
if (isEntrypoint) {
  run().catch((error) => {
    console.error(`alchemy: ${error.message || error}`);
    process.exitCode = 1;
  });
}
