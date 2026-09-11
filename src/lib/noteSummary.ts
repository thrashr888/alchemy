import type { NoteSummary } from "./types";

export function toNoteSummary(note: NoteSummary): NoteSummary {
  if (!("content" in note) && !("prompt" in note)) return note;
  const { id, notebookId, title, kind, origin, status, createdAt, updatedAt } = note;
  return { id, notebookId, title, kind, origin, status, createdAt, updatedAt };
}
