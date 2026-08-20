import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useStore } from "@/lib/store";
import { Markdown } from "./Markdown";
import { PrintPortal } from "./printExport";
import { parseInfographic, PrintInfographic } from "./Infographic";
import { PrintMindMap } from "./MindMap";
import { parseDeck, PrintDeck } from "./SlideDeck";
import { parseCards, PrintCards } from "./Flashcards";

/**
 * The whole surface of a `win-export-*` window (export.rs): render the
 * note's print sheet — the same fixed-ink layouts the in-app PDF buttons
 * use, picked by kind — silently print it to the temp PDF the boot script
 * named, and let the backend ship that PDF (or rasterize it to PNG) and
 * close the window. Anything that doesn't parse as its visual kind falls
 * back to the note's markdown in print typography, so an export never
 * comes back blank.
 */
export function PrintExportView({
  noteId,
  pdfPath,
}: {
  noteId: string;
  pdfPath: string;
}) {
  const notes = useStore((s) => s.notes);
  const appTheme = useStore((s) => s.theme);
  const note = notes.find((n) => n.id === noteId);
  // Slide pages print edge-to-edge on 16:9 landscape paper (print_webview).
  const deck =
    note?.kind === "slide_deck" ? parseDeck(note.content, appTheme) : null;
  const fired = useRef(false);
  useEffect(() => {
    if (!note || fired.current) return;
    fired.current = true;
    const landscape = !!deck;
    // Two frames: one for the print portal to mount, one for layout.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        void invoke("print_webview", { landscape, savePath: pdfPath });
      });
    });
  }, [note, deck, pdfPath]);
  if (!note) return null;

  if (note.kind === "mind_map") return <PrintMindMap content={note.content} />;
  if (deck) return <PrintDeck deck={deck} />;
  if (note.kind === "flashcards") {
    const cards = parseCards(note.content);
    if (cards) return <PrintCards cards={cards} />;
  }
  const doc = parseInfographic(note.content);
  if (note.kind === "infographic" && doc) return <PrintInfographic doc={doc} />;

  // Prose, tables, and every parse fallback: the markdown itself in fixed
  // print ink (print is a document — never the on-screen theme).
  return (
    <PrintPortal pageCss="@page { size: auto; margin: 16mm; }">
      <div
        style={{
          color: "#111",
          background: "#fff",
          fontFamily: "system-ui, sans-serif",
          fontSize: 12,
          maxWidth: 620,
          margin: "0 auto",
          WebkitPrintColorAdjust: "exact",
          printColorAdjust: "exact",
        }}
      >
        <h1 style={{ fontSize: 22, fontWeight: 650, letterSpacing: "-0.01em" }}>
          {note.title}
        </h1>
        <Markdown>{note.content}</Markdown>
      </div>
    </PrintPortal>
  );
}
