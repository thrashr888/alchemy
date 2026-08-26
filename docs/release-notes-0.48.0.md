# Alchemy v0.48.0

## Highlights

**Your library backs itself up every night.** Alchemy clones the whole store
each night and keeps a week of copies plus four weekly ones. On an APFS disk a
clone shares blocks with the original until they differ, so even a multi-gigabyte
library costs almost nothing to keep. Settings → Nightly shows the last snapshot
and restores it.

**A newer library can no longer break an older Alchemy.** The store carries a
version stamp. Open it with an older build and you get a plain screen asking you
to update, instead of a crash. Every upgrade also clones the store aside before
it migrates, so going back is possible.

**Overnight work has one dial.** Light, Standard, or Generous. The app decides
the order: keep sources current, weigh what changed against what you have
concluded, then tidy up. Local models stay free at any setting; the dial caps
what a paid model can spend in a night.

**Alchemy re-reads what changed while you were away.** A watched page that moves
overnight is weighed against the claims in your ledger. When it contradicts
something you recorded in March, that lands in the morning brief rather than
waiting for you to notice.

**Work that ran late says so.** A brief due at 8:00 on a sleeping Mac runs when
the Mac wakes, and now tells you it was due while you were asleep. It reports
what it found rather than what it spent: "found 2 contradictions and refreshed
12 sources." A night that finds nothing says nothing.

## Fixes

- Everything that happens while you are away is on one Settings page, now called
  Nightly. Source refresh and repository sync moved there from Sources, and
  weekly note consolidation from Studio.
- The web clipper lost its switch. The receiver only ever acted when you clicked
  the extension, so the switch governed nothing.
- Chat style options line up in a grid. Long names no longer push the rows out
  of alignment.
- The reports header wraps as a unit instead of splitting "1 of 7" across two
  lines.
- Report schedule menus have room to breathe.
- Archived notebooks stay fully archived. Their scheduled work no longer appears
  anywhere as something needing attention.
