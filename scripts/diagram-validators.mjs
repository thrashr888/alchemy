#!/usr/bin/env node
// Precompile the JSON-schema validators the eraser resolver needs, so the
// app never compiles one at runtime (docs/RFC-diagrams.md, "Content
// Security Policy").
//
// `@eraserlabs/resolve` validates every diagram element with Ajv, and Ajv
// turns a schema into a validator with `new Function`. The release CSP is
// `script-src 'self'`, which forbids that, so a signed build drew no diagram
// at all. This script builds the same two Ajv instances the package builds
// (`schema/compile.js` for the stock library's tag schemas, and
// `schema/definition.js` for the MDP tag-schema meta-schema that checks
// them), compiles the same schemas, and writes the generated functions to
// `src/lib/diagramValidators.gen.js`. At runtime `ajv` is aliased to
// `src/lib/diagramValidators.ts`, which hands those functions back keyed
// by a fingerprint of the schema — nothing is compiled in the WebView.
//
//   pnpm run diagram:validators
//
// Re-run after bumping @eraserlabs/* or ajv; the shim throws at resolver
// warm-up (and src/lib/diagramCsp.test.ts fails) when a schema it sees
// has no precompiled validator.
import { createRequire } from "node:module";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outJs = path.join(root, "src/lib/diagramValidators.gen.js");
const outDts = path.join(root, "src/lib/diagramValidators.gen.d.ts");

// ajv and @eraserlabs/protocol are @eraserlabs/resolve's dependencies, not
// ours, so they resolve from the package's own directory (pnpm keeps them
// out of the top-level node_modules on purpose).
const resolveEntry = import.meta.resolve("@eraserlabs/resolve");
const req = createRequire(resolveEntry);
const load = (id) => import(pathToFileURL(req.resolve(id)).href);
const internal = (file) => import(new URL(file, resolveEntry).href);

const { Ajv } = await load("ajv");
const { default: standaloneCode } = await load("ajv/dist/standalone");
const ajvVersion = JSON.parse(readFileSync(req.resolve("ajv/package.json"), "utf8")).version;
const diagramsVersion = JSON.parse(
  readFileSync(req.resolve("@eraserlabs/diagrams/package.json", { paths: [root] }), "utf8"),
).version;
const { registerMetadataKeywords } = await internal("./schema/keywords.js");
const { assertValidTagSchema } = await internal("./schema/definition.js");
const { elementKindOf, isContainerTag } = await import("@eraserlabs/resolve/schema");
const { prepareLibrary } = await import("@eraserlabs/resolve");
const { stockLibrary } = await import("@eraserlabs/diagrams/library");
const tagSchemaMeta = JSON.parse(
  readFileSync(req.resolve("@eraserlabs/protocol/schemas/tag-schema"), "utf8"),
);

/**
 * Same string on both sides: the shim fingerprints the schema object Ajv
 * would have compiled and looks the validator up by it. The function's own
 * source is emitted into the generated module, so there is one copy of the
 * algorithm.
 */
function fingerprint(schema) {
  const text = JSON.stringify(schema);
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i += 1) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `${text.length}-${hash.toString(16).padStart(8, "0")}`;
}

/** Verbatim from @eraserlabs/resolve/dist/schema/compile.js (not exported there). */
function withEntityIsContainer(schema, kind) {
  if (kind !== "entity") {
    return schema;
  }
  const root = schema;
  const properties = root["properties"];
  if (
    typeof properties !== "object" ||
    properties === null ||
    Array.isArray(properties) ||
    Object.hasOwn(properties, "isContainer")
  ) {
    return schema;
  }
  return {
    ...root,
    properties: {
      ...properties,
      isContainer: {
        type: "boolean",
        ...(isContainerTag(schema) ? { default: true } : {}),
      },
    },
  };
}

// The library exactly as createResolver() prepares it (src/lib/eraserDiagram.ts
// passes stockLibrary with no overrides), then the same Ajv options as
// createAjv() in schema/compile.js, plus `code.source` so the compiled
// functions can be written out.
const library = prepareLibrary(stockLibrary);
const hasPalette = library.palette !== undefined;
const tagAjv = new Ajv({
  allErrors: true,
  strict: false,
  allowUnionTypes: true,
  removeAdditional: false,
  useDefaults: true,
  code: { source: true, lines: true },
});
registerMetadataKeywords(tagAjv);

const tags = {};
for (const [tag, schema] of Object.entries(library.schemas)) {
  assertValidTagSchema(tag, schema, hasPalette);
  const kind = elementKindOf(schema);
  if (!kind) throw new TypeError(`schema for tag "${tag}" must declare x-schema-kind`);
  const compiled = withEntityIsContainer(schema, kind);
  tagAjv.addSchema(compiled, tag);
  tags[tag] = fingerprint(compiled);
}
const tagCode = standaloneCode(tagAjv, Object.fromEntries(Object.keys(tags).map((t) => [t, t])));

// schema/definition.js: `new Ajv({ allErrors: true, strict: true }).compile(tagSchemaMeta)`,
// run once at module load against the MDP meta-schema.
const metaAjv = new Ajv({ allErrors: true, strict: true, code: { source: true, lines: true } });
metaAjv.addSchema(tagSchemaMeta, "tagSchema");
const metaCode = standaloneCode(metaAjv, { tagSchema: "tagSchema" });
const META = "tag-schema";
tags[META] = fingerprint(tagSchemaMeta);

// Standalone output is CommonJS and pulls a couple of helpers from
// `ajv/dist/runtime/*`. Those files contain no eval, but ajv is not our
// dependency and the WebView bundle should not reach into it, so each
// blob runs inside its own module scope with the helpers inlined below.
const RUNTIME = {
  "ajv/dist/runtime/equal": "equal",
  "ajv/dist/runtime/ucs2length": "ucs2length",
};
function moduleScope(code) {
  const wanted = new Set(code.match(/require\("[^"]+"\)/g) ?? []);
  for (const call of wanted) {
    const id = call.slice('require("'.length, -2);
    if (!(id in RUNTIME)) throw new Error(`generated validator needs an unknown runtime helper: ${id}`);
  }
  if (/new Function|\beval\(/.test(code)) throw new Error("generated validator still evaluates code");
  return `(() => {
  const exports = {};
  const require = requireRuntime;
${code.replace(/^"use strict";\n?/, "")}
  return exports;
})()`;
}

const header = `// GENERATED by scripts/diagram-validators.mjs — do not edit.
// Regenerate with: pnpm run diagram:validators
//
// Ajv ${ajvVersion} validators for the @eraserlabs/diagrams ${diagramsVersion} stock
// library (one per tag) and for the MDP tag-schema meta-schema, compiled
// ahead of time so the WebView compiles nothing at runtime: the release CSP
// is \`script-src 'self'\`. src/lib/diagramValidators.ts stands in for ajv
// and serves these by schema fingerprint. See docs/RFC-diagrams.md.
/* eslint-disable */
`;

const helpers = `// Inlined from ajv/dist/runtime (fast-deep-equal and ucs2length, MIT).
function equal(a, b) {
  if (a === b) return true;
  if (a && b && typeof a == "object" && typeof b == "object") {
    if (a.constructor !== b.constructor) return false;
    var length, i, keys;
    if (Array.isArray(a)) {
      length = a.length;
      if (length != b.length) return false;
      for (i = length; i-- !== 0; ) if (!equal(a[i], b[i])) return false;
      return true;
    }
    if (a.constructor === RegExp) return a.source === b.source && a.flags === b.flags;
    if (a.valueOf !== Object.prototype.valueOf) return a.valueOf() === b.valueOf();
    if (a.toString !== Object.prototype.toString) return a.toString() === b.toString();
    keys = Object.keys(a);
    length = keys.length;
    if (length !== Object.keys(b).length) return false;
    for (i = length; i-- !== 0; ) if (!Object.prototype.hasOwnProperty.call(b, keys[i])) return false;
    for (i = length; i-- !== 0; ) {
      var key = keys[i];
      if (!equal(a[key], b[key])) return false;
    }
    return true;
  }
  return a !== a && b !== b;
}
function ucs2length(str) {
  const len = str.length;
  let length = 0;
  let pos = 0;
  let value;
  while (pos < len) {
    length++;
    value = str.charCodeAt(pos++);
    if (value >= 0xd800 && value <= 0xdbff && pos < len) {
      value = str.charCodeAt(pos);
      if ((value & 0xfc00) === 0xdc00) pos++;
    }
  }
  return length;
}
const requireRuntime = (id) => {
  const helper = { ${Object.entries(RUNTIME)
    .map(([id, name]) => `${JSON.stringify(id)}: ${name}`)
    .join(", ")} }[id];
  if (!helper) throw new Error(\`no inlined runtime helper for \${id}\`);
  return { default: helper };
};
`;

const body = `${header}
export ${fingerprint.toString()}

${helpers}
const tagValidators = ${moduleScope(tagCode)};

const metaValidators = ${moduleScope(metaCode)};

/** Tag (or "${META}") → the fingerprint of the schema it was compiled from. */
export const TAGS = Object.freeze(${JSON.stringify(tags, null, 2)});

/** Schema fingerprint → validator, for every schema the resolver compiles. */
export const VALIDATORS = Object.freeze({
${Object.keys(tags)
  .map((t) =>
    t === META
      ? `  [TAGS[${JSON.stringify(t)}]]: metaValidators.tagSchema,`
      : `  [TAGS[${JSON.stringify(t)}]]: tagValidators[${JSON.stringify(t)}],`,
  )
  .join("\n")}
});
`;

const dts = `// GENERATED by scripts/diagram-validators.mjs — do not edit.
// Regenerate with: pnpm run diagram:validators
/** Ajv's ValidateFunction, as far as the resolver uses it (ajv is not our dependency). */
export interface ValidateFunction {
  (data: unknown): boolean;
  errors?: null | ErrorObject[];
}
export interface ErrorObject {
  keyword: string;
  instancePath: string;
  schemaPath: string;
  params: Record<string, unknown>;
  message?: string;
}

/** Tag (or "tag-schema") → the fingerprint of the schema it was compiled from. */
export const TAGS: Readonly<Record<string, string>>;
/** Schema fingerprint → validator, for every schema the resolver compiles. */
export const VALIDATORS: Readonly<Record<string, ValidateFunction>>;
/** The fingerprint of a schema object, as the shim computes it at runtime. */
export function fingerprint(schema: unknown): string;
`;

writeFileSync(outJs, body);
writeFileSync(outDts, dts);
console.log(
  `wrote ${path.relative(root, outJs)} (${(body.length / 1024).toFixed(1)} KB): ${Object.keys(tags).length - 1} tag validators + the tag-schema meta-schema`,
);
