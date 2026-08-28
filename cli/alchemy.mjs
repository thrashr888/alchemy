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
  alchemy notebooks [--json]
  alchemy add <file-or-url>... --notebook <id-or-title> [--title <title>] [--json]
  alchemy add - --notebook <id-or-title> [--title <title>] [--json]
  alchemy search <query...> [--notebook <id-or-title>] [--limit <1-20>] [--json]

Commands:
  notebooks  List notebooks and their ids.
  add        Add local files, web URLs, or stdin (use - or pipe with no input).
  search     Search one notebook, or all notebooks when --notebook is omitted.

Connection:
  The Alchemy app must be running with MCP enabled. The CLI discovers the
  app through its mcp.json file. Override with --mcp-url or ALCHEMY_MCP_URL.

Install from this checkout:
  pnpm add --global ./cli

Examples:
  alchemy notebooks
  alchemy add report.pdf https://example.com --notebook "Project Atlas"
  pbpaste | alchemy add --notebook "Project Atlas" --title "Meeting notes"
  alchemy search "renewal risk" --notebook "Project Atlas"
  alchemy search "where did I save the contractor agreement?" --json`;

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
  if (args.length === 0 || args[0] === "help" || takeFlag(args, "--help")) {
    return { command: "help" };
  }
  if (takeFlag(args, "--version")) return { command: "version" };

  const command = args.shift();
  const mcpUrl = takeOption(args, "--mcp-url");
  const json = takeFlag(args, "--json");

  if (command === "notebooks") {
    if (args.length) throw new CliError(`unexpected argument: ${args[0]}`);
    return { command, mcpUrl, json };
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
    return { command, mcpUrl, json, notebook, title, inputs: args };
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
    return { command, mcpUrl, json, notebook, query, limit };
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
  if (explicit) return validateMcpUrl(explicit, "--mcp-url");
  if (env.ALCHEMY_MCP_URL) return validateMcpUrl(env.ALCHEMY_MCP_URL, "ALCHEMY_MCP_URL");

  for (const path of discoveryPaths(env)) {
    try {
      const info = JSON.parse(await readFile(path, "utf8"));
      if (typeof info.url === "string") return validateMcpUrl(info.url, path);
      if (Number.isInteger(info.port)) {
        return `http://127.0.0.1:${info.port}/mcp`;
      }
      throw new CliError(`Alchemy discovery file has no url or port: ${path}`);
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      if (error instanceof CliError) throw error;
      throw new CliError(`could not read Alchemy discovery file ${path}: ${error.message}`);
    }
  }
  return DEFAULT_MCP_URL;
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
  constructor(url, fetchImpl = globalThis.fetch) {
    this.url = url;
    this.fetch = fetchImpl;
    this.sessionId = null;
    this.nextId = 1;
  }

  async post(body, sessionId = this.sessionId) {
    const headers = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
    };
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
    if (!response.ok) throw new CliError(`Alchemy MCP initialize failed (HTTP ${response.status})`);
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

  const client = new McpClient(await discoverMcpUrl(options.mcpUrl));
  if (options.command === "notebooks") {
    const notebooks = await client.call("list_notebooks");
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
