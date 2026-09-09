/**
 * The `architecture` artifact's document (docs/RFC-diagrams.md): the
 * eraser-diagrams split form — `{ entities, connections }` of tagged
 * objects — minus coordinates. The generator writes topology (groups,
 * components, the lines between them) and `diagramLayout.ts` places it;
 * any `x`/`y` the model volunteers is ignored, because a model guessing
 * pixel positions is the one thing this pipeline exists to avoid.
 *
 * Everything past the envelope is eraser's vocabulary verbatim, so the
 * document a note holds is also a valid input to the upstream CLI.
 */

export type ArchDirection = "down" | "right";

export interface ArchEntity {
  tag: string;
  id: string;
  /** Group/Lane/Pool this entity sits inside. */
  containerId?: string | null;
  [prop: string]: unknown;
}

export interface ArchConnection {
  from: string;
  to: string;
  tag?: string;
  id?: string;
  [prop: string]: unknown;
}

export interface ArchDoc {
  title?: string;
  /** Main flow axis: `down` stacks tiers as rows, `right` as columns. */
  direction: ArchDirection;
  entities: ArchEntity[];
  connections: ArchConnection[];
}

/** The stock tags that hold other entities. */
export const CONTAINER_TAGS: ReadonlySet<string> = new Set(["Group", "Lane", "Pool"]);

/**
 * The JSON text inside a generated note. Models wrap it in a fence or
 * preface it with a sentence despite the instruction; neither should cost
 * the diagram.
 */
export function architectureSource(content: string): string {
  const fenced = /```(?:json)?\s*\n([\s\S]*?)```/.exec(content);
  const body = (fenced ? fenced[1] : content).trim();
  const start = body.indexOf("{");
  const end = body.lastIndexOf("}");
  if (start < 0 || end <= start) return body;
  return body.slice(start, end + 1).trim();
}

export type ArchParse = { doc: ArchDoc; error?: never } | { doc?: never; error: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Parse a note's content into a document, or say what is wrong with it. */
export function parseArchitecture(content: string): ArchParse {
  const source = architectureSource(content);
  let raw: unknown;
  try {
    raw = JSON.parse(source);
  } catch (e) {
    return { error: `not valid JSON: ${e instanceof Error ? e.message : String(e)}` };
  }
  if (!isRecord(raw)) return { error: "the document must be a JSON object" };
  if (!Array.isArray(raw.entities)) return { error: 'the document needs an "entities" array' };
  // A forgotten `connections` is a diagram with no lines, not no diagram.
  const rawConnections = raw.connections ?? [];
  if (!Array.isArray(rawConnections)) return { error: '"connections" must be an array' };

  const entities: ArchEntity[] = [];
  for (const [i, entity] of raw.entities.entries()) {
    if (!isRecord(entity)) return { error: `entity ${i} is not an object` };
    if (typeof entity.tag !== "string" || !entity.tag)
      return { error: `entity ${i} has no "tag"` };
    if (typeof entity.id !== "string" || !entity.id)
      return { error: `entity ${i} (${entity.tag}) has no "id"` };
    // Placement is ours; a model's coordinates are noise.
    const { x: _x, y: _y, width: _w, height: _h, ...rest } = entity;
    entities.push(rest as ArchEntity);
  }

  const connections: ArchConnection[] = [];
  for (const [i, connection] of rawConnections.entries()) {
    if (!isRecord(connection)) return { error: `connection ${i} is not an object` };
    if (typeof connection.from !== "string" || typeof connection.to !== "string")
      return { error: `connection ${i} needs "from" and "to"` };
    connections.push(connection as ArchConnection);
  }

  return {
    doc: {
      ...(typeof raw.title === "string" ? { title: raw.title } : {}),
      direction: raw.direction === "right" ? "right" : "down",
      entities,
      connections,
    },
  };
}

/** The document as the note stores it: stable key order, readable indent. */
export function formatArchitecture(doc: ArchDoc): string {
  return JSON.stringify(doc, null, 2);
}
