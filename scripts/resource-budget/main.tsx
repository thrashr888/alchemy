import React from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { api } from "../../src/lib/api";
import { toNoteSummary } from "../../src/lib/noteSummary";
import type { Note } from "../../src/lib/types";
import "../../src/index.css";

const nativeHost = "__TAURI_INTERNALS__" in window;
const calls: string[] = [];
const notes = new Map<string, Note>(["a", "b", "table"].map((id) => [id, {
  id, notebookId: "nb", title: `Report ${id}`, kind: "report", origin: "", status: "",
  content: id === "table" ? "| A | B |\n|---|---|\n| 1 | 2 |" : `Original body ${id}`,
  prompt: `Preserve instructions ${id}`, createdAt: 1, updatedAt: 1,
}]));
let slow = "";
let failure = "";
let release: () => void = () => {};
let exported = "";
let deleted = 0;
const restores: Note[] = [];
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const fixtureIPC = async (command: string, args: unknown) => {
  const p = args as Record<string, any>;
  calls.push(command);
  if (command === "read_note") {
    if (p.noteId === failure) throw new Error("Read failed");
    const snapshot = structuredClone(notes.get(p.noteId));
    if (p.noteId === slow) await new Promise<void>((r) => { release = r; });
    else await sleep(30);
    if (!snapshot) throw new Error("Note not found");
    return snapshot;
  }
  if (command === "list_note_summaries") return [...notes.values()].map(toNoteSummary);
  if (command === "create_note") {
    const note = { ...notes.get("b")!, id: "new", title: p.title, content: p.content };
    notes.set(note.id, note); return note;
  }
  if (command === "update_note") {
    const note = notes.get(p.id)!;
    Object.assign(note, { title: p.title, content: p.content, updatedAt: note.updatedAt + 1 });
    return;
  }
  if (command === "delete_notes") { deleted++; for (const id of p.ids) notes.delete(id); return; }
  if (command === "restore_note") {
    const note = { ...p.note, id: `restored-${restores.length}`, createdAt: 1, updatedAt: 2 };
    restores.push(note); notes.set(note.id, note); return note;
  }
  if (command === "plugin:dialog|save") return `/tmp/fixture.${p.options.filters[0].extensions[0]}`;
  if (command === "export_note") { exported = p.format; return p.dest; }
  if (command === "related_passages") return [];
  return null;
};
if (nativeHost) {
  // Native Tauri internals are readonly. Override only our API facade here.
  api.readNote = (noteId) => fixtureIPC("read_note", { noteId });
  api.relatedPassages = async () => [];
} else {
  mockWindows("main");
  mockIPC(fixtureIPC, { shouldMockEvents: true });
}
const { useStore } = await import("../../src/lib/store");
const { ReaderPane } = await import("../../src/components/ReaderPane");
const { DitherBackground } = await import("../../src/components/DitherBackground");
const { TileShader } = await import("../../src/components/settings/TileShader");
const { exportNote, exportTargets } = await import("../../src/lib/noteExport");
const root = createRoot(document.getElementById("root")!);
useStore.setState({ currentId: "nb", notes: [...notes.values()].map(toNoteSummary) });
const open = (id: string) => useStore.setState({ reader: { open: true, history: [{ type: "note", id }], index: 0 } });
let drawCount = 0;
const originalDraw = WebGLRenderingContext.prototype.drawArrays;
WebGLRenderingContext.prototype.drawArrays = function(...args) { drawCount++; return originalDraw.apply(this, args); };
function reader() { root.render(<div style={{ display: "flex", height: "100vh", width: 1000 }}><ReaderPane /></div>); }
function shaders() { root.render(<><div style={{ height: 400, position: "relative" }}><DitherBackground themeKey="dracula" /><TileShader mode="ember" tintVar="--primary" /></div><div style={{ height: 2200 }} /></>); }
reader(); open("b");
Object.assign(window, { resourceBudget: {
  calls, notes, restores, open, reader, shaders,
  get draws() { return drawCount; }, get exported() { return exported; }, get deleted() { return deleted; },
  slow: (id: string) => { slow = id; }, fail: (id: string) => { failure = id; },
  release: () => { slow = ""; release(); },
  refresh: () => useStore.setState({ notes: [...notes.values()].map(toNoteSummary) }),
  create: () => useStore.getState().createNote("Big new note", "X".repeat(1_000_000)),
  delete: (ids: string[]) => useStore.getState().deleteNotesBatch(ids),
  undo: () => useStore.getState().undoLast(), redo: () => useStore.getState().redoLast(),
  state: () => ({ notes: useStore.getState().notes, error: useStore.getState().error }),
  export: async (id: string) => { const row = toNoteSummary(notes.get(id)!); await exportNote(row, exportTargets(row)[0]); },
  close: () => root.render(null), api,
} });

// The same production components can run in an isolated Tauri window. The
// optional local collector records native WebKit results without a debugger.
if (nativeHost) {
  void (async () => {
    await sleep(1000);
    const readerRendered = document.body.textContent?.includes("Original body b");
    shaders(); await sleep(500);
    const start = drawCount; await sleep(1000);
    const visibleDraws = drawCount - start;
    window.scrollTo(0, 900); await sleep(100);
    const hiddenStart = drawCount; await sleep(1000);
    const hiddenDraws = drawCount - hiddenStart;
    window.scrollTo(0, 0); await sleep(200);
    const resumed = drawCount > hiddenStart;
    const result = { readerRendered, visibleDraws, hiddenDraws, resumed,
      renderer: navigator.userAgent,
      compiledCanvases: [...document.querySelectorAll("canvas")].filter((c) => c.style.display !== "none").length };
    document.title = "Resource checks " + JSON.stringify(result);
    await fetch("http://127.0.0.1:8796/", { method: "POST", body: JSON.stringify(result) });
  })();
}
