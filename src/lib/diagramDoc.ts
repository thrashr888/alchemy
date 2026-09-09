/**
 * The eraser-diagrams artifacts' document (docs/RFC-diagrams.md): the
 * split form — `{ entities, connections }` of tagged objects — minus
 * coordinates. The generator writes topology (groups, components, the
 * lines between them) and `diagramLayout.ts` places it; any `x`/`y` the
 * model volunteers is ignored, because a model guessing pixel positions is
 * the one thing this pipeline exists to avoid.
 *
 * Five artifact kinds share the envelope and differ in vocabulary: each
 * kind admits the stock tags its prompt teaches and nothing else, so a
 * data model cannot quietly turn into an architecture diagram. Everything
 * past the envelope is eraser's vocabulary verbatim, so the document a
 * note holds is also a valid input to the upstream CLI.
 */

export type DiagramDirection = "down" | "right";

/** The artifact kinds eraser renders, in Studio order. */
export const DIAGRAM_KINDS = [
  "architecture",
  "process",
  "data_model",
  "relationship",
  "journey",
] as const;
export type DiagramKind = (typeof DIAGRAM_KINDS)[number];

export function isDiagramKind(kind: string): kind is DiagramKind {
  return (DIAGRAM_KINDS as readonly string[]).includes(kind);
}

/** What the title chip says when the document has no title of its own. */
export const DIAGRAM_KIND_LABEL: Record<DiagramKind, string> = {
  architecture: "Architecture",
  process: "Process map",
  data_model: "Data model",
  relationship: "Relationship map",
  journey: "Journey map",
};

/**
 * The stock tags each kind may use — entity tags and connection tags. The
 * prompt in rag.rs (`DIAGRAM_TAGS`) teaches exactly these; a test holds
 * the two lists in lockstep.
 */
export const KIND_TAGS: Record<DiagramKind, { entities: readonly string[]; connections: readonly string[] }> = {
  architecture: {
    entities: ["Group", "Lane", "Pool", "Icon", "Shape", "Textbox"],
    connections: ["Relationship"],
  },
  process: {
    entities: ["Pool", "Lane", "Activity", "Event", "Gateway", "Textbox"],
    connections: ["Relationship"],
  },
  data_model: {
    entities: ["Group", "DatabaseTable", "Textbox"],
    connections: ["Relationship", "DatabaseRelationship"],
  },
  relationship: {
    entities: ["Group", "Icon", "Shape", "Textbox"],
    connections: ["Relationship"],
  },
  journey: {
    entities: ["Group", "Shape", "Textbox", "Event"],
    connections: ["Relationship"],
  },
};

export interface DiagramEntity {
  tag: string;
  id: string;
  /** Group/Lane/Pool this entity sits inside. */
  containerId?: string | null;
  [prop: string]: unknown;
}

export interface DiagramConnection {
  from: string;
  to: string;
  tag?: string;
  id?: string;
  [prop: string]: unknown;
}

export interface DiagramDoc {
  title?: string;
  /** Main flow axis: `down` stacks tiers as rows, `right` as columns. */
  direction: DiagramDirection;
  entities: DiagramEntity[];
  connections: DiagramConnection[];
}

/** The stock tags that hold other entities. */
export const CONTAINER_TAGS: ReadonlySet<string> = new Set(["Group", "Lane", "Pool"]);

/** Each kind's natural main axis when the document names none. */
const DEFAULT_DIRECTION: Record<DiagramKind, DiagramDirection> = {
  architecture: "down",
  process: "down", // lanes as rows, steps left to right
  data_model: "right",
  relationship: "right",
  journey: "right", // stages as columns
};

/**
 * The JSON text inside a generated note. Models wrap it in a fence or
 * preface it with a sentence despite the instruction; neither should cost
 * the diagram.
 */
export function diagramSource(content: string): string {
  const fenced = /```(?:json)?\s*\n([\s\S]*?)```/.exec(content);
  const body = (fenced ? fenced[1] : content).trim();
  const start = body.indexOf("{");
  const end = body.lastIndexOf("}");
  if (start < 0 || end <= start) return body;
  return body.slice(start, end + 1).trim();
}

export type DiagramParse =
  | { doc: DiagramDoc; error?: never; warnings: string[] }
  | { doc?: never; error: string; warnings?: never };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Parse a note's content into a document for `kind`, or say what is wrong with it. */
export function parseDiagram(content: string, kind: DiagramKind): DiagramParse {
  const source = diagramSource(content);
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
  const allowed = KIND_TAGS[kind];

  const entities: DiagramEntity[] = [];
  for (const [i, entity] of raw.entities.entries()) {
    if (!isRecord(entity)) return { error: `entity ${i} is not an object` };
    if (typeof entity.tag !== "string" || !entity.tag)
      return { error: `entity ${i} has no "tag"` };
    if (typeof entity.id !== "string" || !entity.id)
      return { error: `entity ${i} (${entity.tag}) has no "id"` };
    if (!allowed.entities.includes(entity.tag))
      return {
        error: `entity "${entity.id}" has tag ${entity.tag}, which a ${DIAGRAM_KIND_LABEL[kind].toLowerCase()} does not use (its tags: ${allowed.entities.join(", ")})`,
      };
    // Placement is ours; a model's coordinates are noise.
    const { x: _x, y: _y, width: _w, height: _h, ...rest } = entity;
    entities.push(rest as DiagramEntity);
  }

  const warnings: string[] = [];
  const tagOf = new Map(entities.map((e) => [e.id, e.tag]));
  const connections: DiagramConnection[] = [];
  for (const [i, connection] of rawConnections.entries()) {
    if (!isRecord(connection)) return { error: `connection ${i} is not an object` };
    if (typeof connection.from !== "string" || typeof connection.to !== "string")
      return { error: `connection ${i} needs "from" and "to"` };
    if (typeof connection.tag === "string" && !allowed.connections.includes(connection.tag))
      return {
        error: `connection ${i} has tag ${connection.tag}, which a ${DIAGRAM_KIND_LABEL[kind].toLowerCase()} does not use (its tags: ${allowed.connections.join(", ")})`,
      };
    // A relationship map's Textbox is a note beside the map, not a member
    // of it: a line into one is a relation the model had no entity for.
    // The rest of the map is still right, so the line goes, with a word.
    if (kind === "relationship") {
      const note = [connection.from, connection.to].find((id) => tagOf.get(id) === "Textbox");
      if (note !== undefined) {
        warnings.push(
          `connection "${connection.from}" → "${connection.to}" dropped: "${note}" is a Textbox, a note beside the map, not something a relationship map connects`,
        );
        continue;
      }
    }
    connections.push(connection as DiagramConnection);
  }

  const direction =
    raw.direction === "right" || raw.direction === "down"
      ? raw.direction
      : DEFAULT_DIRECTION[kind];
  return {
    doc: {
      ...(typeof raw.title === "string" ? { title: raw.title } : {}),
      direction,
      entities,
      connections,
    },
    warnings,
  };
}

/** The document as the note stores it: stable key order, readable indent. */
export function formatDiagram(doc: DiagramDoc): string {
  return JSON.stringify(doc, null, 2);
}

const CARDINALITY = /^\s*([01n*m]|many|one)\s*(?:\.\.|:|-to-|\s+to\s+|-|—)\s*([01n*m]|many|one)\s*$/i;

function relTypeOf(label: unknown): string | undefined {
  const m = typeof label === "string" ? CARDINALITY.exec(label) : null;
  if (!m) return undefined;
  const many = (s: string) => /^[nm*]$|^many$/i.test(s);
  return `${many(m[1]) ? "many" : "one"}-to-${many(m[2]) ? "many" : "one"}`;
}

/**
 * The document as the renderer wants it. The stored note is the model's
 * text verbatim; this derives what a kind implies at render time and never
 * writes it back. For a data model, a connection whose label reads as a
 * cardinality ("1..n", "n..n", "one-to-many") becomes a DatabaseRelationship,
 * so the crow's feet draw from the label the sources gave — the label
 * itself stays.
 */
export function prepareForRender(doc: DiagramDoc, kind: DiagramKind): DiagramDoc {
  if (kind !== "data_model") return doc;
  return {
    ...doc,
    connections: doc.connections.map((c) => {
      if (c.tag === "DatabaseRelationship" || typeof c.relType === "string") {
        return { ...c, tag: "DatabaseRelationship" };
      }
      const relType = relTypeOf(c.label);
      return relType ? { ...c, tag: "DatabaseRelationship", relType } : c;
    }),
  };
}
