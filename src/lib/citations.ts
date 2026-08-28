import { navAtomic, useStore } from "./store";
import type { MetaCitation } from "./types";

/**
 * Jump to a passage behind a corpus-wide answer (docs/RFC-meta-chat.md):
 * select the citation's notebook, then open the note card or the source
 * reader at the snippet — the same routing ⌘K search hits use. Registry
 * cards are corpus-scoped and carry no notebook, so they open on Home.
 *
 * Shared by the ⌘K palette's ask mode and the Home chat; callers dismiss
 * their own surface first (the palette closes, the Home chat stays).
 *
 * The whole jump is ONE history entry (`navAtomic`). Opening a source takes
 * two store writes — select the notebook, then open the reader — and each was
 * recorded separately, so Back out of a cited source landed on that
 * notebook's chat rather than on the conversation you asked from.
 */
export async function openMetaCitation(c: MetaCitation): Promise<void> {
  await navAtomic(async () => {
    const s = useStore.getState();
    if (c.kind === "card") {
      s.closeNotebook();
      useStore.setState({ homeSection: "registry", openCardId: c.id });
    } else if (c.kind === "note") {
      // StudioPanel auto-opens this id once the notebook's notes load.
      useStore.setState({ justCreatedNoteId: c.id });
      if (!s.studioOpen) s.toggleStudio();
      await s.selectNotebook(c.notebookId);
    } else {
      await s.selectNotebook(c.notebookId);
      useStore.getState().openSourceViewer(c.id, c.title, c.snippet);
    }
  });
}
