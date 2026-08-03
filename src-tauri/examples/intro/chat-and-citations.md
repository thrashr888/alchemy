Chat in Alchemy is grounded: every answer is built from numbered excerpts retrieved out of your actual sources, and the citations under an answer point at the exact passages used. This source explains the loop and the tools around it.

## How an answer happens

When you ask a question, Alchemy embeds it and runs a hybrid search over your selected sources — semantic vector search and keyword (BM25) search, merged by reciprocal rank fusion, so it catches both "means the same thing" matches and exact-term matches like part numbers or names. The best excerpts are assembled into a numbered prompt, your chat model streams an answer, and the citations are saved with the message.

**Click any citation** and the source opens in the Reader, scrolled to the cited passage and highlighted. That round trip — claim to evidence in one click — is the whole point. You can copy a response or save it as a note in one action, and each answer is captioned with the model that wrote it (and its metered cost, when the provider reports one).

## Steering retrieval

- **Source checkboxes** in the left rail control the retrieval pool.
- **@ mentions** — type @ to name a source, folder, or note; that one question retrieves only from what you named.
- **Slash commands** — type / in the composer for a picker over every command: all the document generators (trailing text becomes custom instructions), plus /add, /model, /research, /grep, /note, /report, and /clear. Fuzzy matching tolerates "study guide" for study_guide, and Tab completes.
- **/grep** runs an exact-match text search across your sources inline — no model call, nothing saved to the transcript. Ideal for "does this phrase appear anywhere?"
- **Deep research** — an agentic mode that plans multiple retrieval steps for questions a single search can't answer well.

## Reading and asking from the Reader

Sources and notes open in the center column, never a modal. Navigate with browser-style back/forward (⌘[ / ⌘]), step through the notebook with j/k, and search inside a document with ⌘F. Markdown renders properly, code files get syntax highlighting, and relative links between sources resolve wiki-style.

Highlight any passage in the Reader to **Explain** it, **Compare** it against your other sources, or stage it in the composer with your own question attached.

## Asking across every notebook

The ⌘K palette and the home screen's ask box answer questions across **all** notebooks at once: Alchemy routes semantically to the likely notebooks, retrieves with diversity caps so one source can't crowd out the rest, and streams an answer with notebook chips and citations that jump straight to the passage. Summon it from anywhere with ⌥Space. Broad questions — "summarize the themes", "what do these sources disagree on?" — are answered across every relevant source, not just the closest few passages.

## Checking the work

For answers you're about to rely on, **Second Look** re-verifies claims against the sources using a different engine than the one that wrote the answer, returning per-claim verdicts. Trust, but verify — cheaply.

Try it now: ask this notebook "What file formats can I import?" and click the citations in the answer.
