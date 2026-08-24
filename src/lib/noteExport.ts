import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "./api";
import { useStore } from "./store";
import type { Note } from "./types";

/* Per-note export (docs/RFC-note-export.md), shared by the Studio panel and
 * the Notebook menu's Export Note… — lifted out of StudioPanel so lib code
 * can drive it without importing a component module. */

/** True when the note body is essentially one markdown table. */
export function isTabular(content: string): boolean {
  const lines = content
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  const tableLines = lines.filter((line) => line.startsWith("|")).length;
  return tableLines >= 2 && tableLines >= lines.length - 2;
}

export interface ExportTarget {
  label: string;
  format: string;
  ext: string;
  name: string;
}

const PDF_TARGET: ExportTarget = {
  label: "Export PDF",
  format: "pdf",
  ext: "pdf",
  name: "PDF",
};

/** The export formats matched to the note's shape (docs/RFC-note-export.md):
 *  a kind-true primary — poster image, slide deck, workbook, episode audio,
 *  Word document — plus a PDF of the note's own render for everything the
 *  print pipeline covers (audio stays audio). */
export function exportTargets(n: Note): ExportTarget[] {
  if (n.kind === "infographic" || n.kind === "mind_map")
    return [
      { label: "Export PNG", format: "png", ext: "png", name: "PNG image" },
      PDF_TARGET,
    ];
  if (n.kind === "slide_deck" || n.kind === "flashcards")
    return [
      { label: "Export PowerPoint", format: "pptx", ext: "pptx", name: "PowerPoint" },
      PDF_TARGET,
    ];
  if (n.kind === "audio_overview")
    return [{ label: "Export audio", format: "m4a", ext: "m4a", name: "Audio" }];
  if (n.kind === "data_table" || isTabular(n.content))
    return [
      { label: "Export Excel", format: "xlsx", ext: "xlsx", name: "Excel workbook" },
      PDF_TARGET,
    ];
  return [
    { label: "Export Word", format: "docx", ext: "docx", name: "Word document" },
    PDF_TARGET,
  ];
}

/** Pick a destination in the native save dialog, export, reveal in Finder. */
export async function exportNote(n: Note, t: ExportTarget): Promise<void> {
  const safe = n.title.replace(/[/\\:]/g, "-").trim() || "Note";
  const dest = await save({
    defaultPath: `${safe}.${t.ext}`,
    filters: [{ name: t.name, extensions: [t.ext] }],
  });
  if (!dest) return;
  const { pushToast } = useStore.getState();
  pushToast("info", "Exporting…");
  try {
    const path = await api.exportNote(n.id, t.format, dest);
    pushToast("success", "Exported");
    void revealItemInDir(path);
  } catch (e) {
    pushToast("error", e instanceof Error ? e.message : String(e));
  }
}
