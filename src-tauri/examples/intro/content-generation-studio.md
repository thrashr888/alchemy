The Studio rail turns a notebook's sources into finished documents. Every generator is one click (or one slash command), takes optional custom instructions, and can be **rebuilt** later against your latest sources — a generated document is a living view of the notebook, not a one-time export.

## The generators

- **Summary, FAQ, Study guide, Briefing, Timeline** — the workhorses for getting oriented in a corpus.
- **Insights** — cross-source connections, contradictions, and surprises.
- **Problems** — hunts for errors, gaps, and contradictions across your sources.
- **Data table** — tabular extraction across sources.
- **Round table** — product, engineering, and design perspectives critique the sources from their own risk lenses, then close on agreements and open questions.
- **Flashcards and Quiz** — study artifacts, described below.
- **Slide deck and Mind map** — visual artifacts, also below.
- **PRD, PR/FAQ, RFC** — structured product and engineering documents in the classic formats.
- **Skill** — generates a SKILL.md so an agent can carry your notebook's know-how.

## Artifacts are real, not text dumps

- **Flashcards** are a flippable deck with Leitner spaced repetition: grade yourself and cards return on a 1/3/7/21-day schedule, persisted per deck.
- **Quizzes** grade each answer against the key's explanation and keep a running score.
- **Slide decks** are Marp-style markdown rendered as true 16:9 slides — layouts inferred per slide, color themes drawn from the app's own theme catalog, a fullscreen Present mode, and one-click PDF export through the native print system.
- **Mind maps** are native SVG on a pannable canvas; open one in its own window for a full-size view.

## Audio Overview

One click turns the notebook into a two-host podcast episode. Your chat model writes the script; the voices are synthesized **entirely on-device** by Kokoro neural TTS (a one-time model download of roughly 93 MB). The script stays readable and editable as a note, and the episode plays inline. It is a genuinely good way to absorb a notebook while doing something else.

## Notes

Notes live alongside generated documents in the Studio rail. The editor is WYSIWYG with Markdown underneath. Two features worth knowing:

- **Convert to source** folds a note into the retrievable source set, so your own synthesis becomes searchable and citable like anything else.
- Chat answers can be saved as notes in one click, citations included.

## Reports on a schedule

A notebook can refresh its URL sources and write a timestamped report note on an interval; each run sees the previous report and calls out what changed since. There is also the **Morning Brief** — a daily arrival note that collects what happened across your notebooks overnight. Alchemy keeps working from the menu bar even with every window closed, so scheduled reports and source re-syncs run on time.

## Templates and instructions

Every generator accepts custom instructions ("keep it under a page", "focus on the financials", "write for an executive audience"), and templates let you save your own document shapes for reuse. If you generate something against this example notebook right now — try a Briefing — you'll see the whole pipeline in about a minute.
