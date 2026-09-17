import { api } from "./api";
import type { RegistryCard } from "./types";

/** Recreate deleted cards — identifiers, note, facts — and re-file their
 *  attachments. The undo half of every card removal (DESIGN.md §9: undo
 *  beats confirm). Ruling metadata (origin/triage) doesn't survive, which
 *  only matters for suggested cards, and those are dismissed, not deleted. */
export async function restoreRegistryCards(cards: RegistryCard[]) {
  for (const c of cards) {
    const restored = await api.addRegistryCard(
      c.kind,
      c.name,
      c.identifiers,
      c.note,
      c.facts,
    );
    for (const a of c.attachments) {
      await api.attachSourceToCard(restored.id, a.sourceId, a.status);
    }
  }
}
