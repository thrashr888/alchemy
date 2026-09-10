/**
 * What `@eraserlabs/resolve` gets when it imports `ajv` (docs/RFC-diagrams.md,
 * "Content Security Policy").
 *
 * The resolver compiles a JSON-schema validator per tag at warm-up, and its
 * schema-definition check compiles the MDP meta-schema at module load. Real
 * Ajv does both with `new Function`, which the release CSP (`script-src
 * 'self'`) forbids — so a signed build drew no diagram at all. This class is
 * the subset of Ajv's surface those two modules touch, backed by validators
 * that `scripts/diagram-validators.mjs` compiled ahead of time from the same
 * schemas with the same options. `compile` looks a schema up by fingerprint;
 * an unknown schema means the library or the resolver changed and the
 * generated module is stale, so it throws with the command that fixes it
 * rather than compiling anything here. Vite aliases the bare `ajv` specifier
 * to this file (see scripts/diagram-csp-alias.ts); nothing else in the app's
 * graph imports ajv.
 */
import { VALIDATORS, fingerprint, type ValidateFunction } from "./diagramValidators.gen.js";

interface KeywordDefinition {
  keyword: string;
}

export class Ajv {
  private readonly keywords = new Set<string>();

  // The options (allErrors, strict, useDefaults, …) shaped the generated
  // code at build time; here they are the caller's business only.
  constructor(_options?: object) {}

  getKeyword(keyword: string): boolean {
    return this.keywords.has(keyword);
  }

  addKeyword(definition: KeywordDefinition): this {
    this.keywords.add(definition.keyword);
    return this;
  }

  compile(schema: object): ValidateFunction {
    const validator = VALIDATORS[fingerprint(schema)];
    if (!validator) {
      const title =
        typeof (schema as { title?: unknown }).title === "string"
          ? (schema as { title: string }).title
          : ((schema as { properties?: { tag?: { const?: unknown } } }).properties?.tag?.const ??
            "unknown");
      throw new Error(
        `no precompiled diagram validator for schema "${String(title)}": the eraser library or resolver changed — run \`pnpm run diagram:validators\` and commit src/lib/diagramValidators.gen.js`,
      );
    }
    return validator;
  }
}

export default Ajv;
