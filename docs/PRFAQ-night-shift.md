# PRFAQ: Night Shift, the area

Draft copy for the Night Shift area concept — the promotion of Night Shift
from background scheduler to a third top-level area beside Notebooks and
Registry. Mocks: [v12-mockups/](v12-mockups/README.md) screens 10–12. Design
grounding: [RFC-night-shift.md](RFC-night-shift.md) and
[RFC-v12-steward.md](RFC-v12-steward.md). Per the claims rules in WRITING.md,
this draft carries no benchmark numbers; measured figures join the copy when
the feature ships and gets measured.

## Press release

### Finished by morning.

**Alchemy's Night Shift becomes a place: hand your Mac the slow work at
night, and review what it did over coffee.**

Alchemy, the local-first research notebook for macOS, today added Night
Shift as a third area of the app, beside Notebooks and Registry. Notebooks
hold your documents. Registry holds your things. Night Shift holds your
time: the work you have decided should happen without you.

Research has a shape problem — the most useful jobs are the slow ones.
Reading forty sources properly takes an evening. Re-checking every claim in
a draft takes an afternoon. Watching a page for the one change that matters
takes forever, which is why nobody does it. Until now the answer was to sit
with the spinner or skip the work.

Night Shift gives that work somewhere to go. The **Tonight** view is where
you leave instructions before bed: rebuild the Japan trip summary from all
its sources, re-check the claims in Thursday's draft, summarize whatever
changed this week. Type it the way you'd ask a colleague. Your Mac does it
while you sleep, with local models by default and a spending cap when a job
uses a paid one.

**Standing orders** are the instructions that repeat. Scheduled reports,
watched pages, and standing questions like "when the 10-K drops, tell me
what changed" live in one list, each with its history and its next run.
Set one in a minute; it works for months.

**The record** is the morning after. Every run leaves a receipt: what was
read, what was written, what it cost, and which model touched it. Notebooks
you mark private show a sealed run that never left the machine. Nothing
Night Shift does is hidden in a log file; the receipt is the interface.

"The app was already good at answering questions while you watch," said
Paul Thrasher, who builds Alchemy. "Night Shift makes the hours you aren't
watching count for something."

Night Shift writes notes and reports. It does not send, post, file, or buy
anything, and every result waits for you to read it.

## FAQ

**What kinds of work can I hand it?**
Anything Alchemy can already do at your desk: deep reads across many
sources, document rebuilds, claim checks on a draft, summaries of what
changed. If a job needs something Night Shift can't do, it says so when you
commission it, not the next morning.

**Does my Mac have to stay on?**
Night Shift runs while your Mac is awake, including with the app window
closed; it lives in the menu bar. If the Mac sleeps through a scheduled
time, the work runs on wake. Nothing is lost, it is late.

**What does it cost to run?**
Local models cost nothing. If a job routes to a paid model, Tonight shows
an estimate before you commit and a nightly cap you set. At the cap, work
continues on local models instead of stopping.

**What about my private notebooks?**
A notebook marked "never leaves this Mac" is honored at the routing layer:
no overnight job may send its contents to a cloud model. The morning
receipt shows it as a sealed run, so the promise is checkable, not assumed.

**Can it act on my behalf?**
No. Night Shift writes notes and reports inside Alchemy. It will not act
outward: no email, no purchases, no file changes outside the app, no
messages sent. Where a result needs action, it proposes and waits.

**How is this different from the scheduled reports Alchemy already has?**
Scheduled reports are one kind of standing order, and they keep working
unchanged. New here: one-off jobs you hand over in plain language, standing
questions that fire on change rather than on a clock, and receipts for
everything.

**What happens when a job fails?**
The receipt says so, plainly, with the reason. Failed scheduled work tries
again on its next run; a failed commission waits for you to re-send or drop
it.

**My fans spun up during a meeting. How do I stop it?**
"Pause until morning" is one click in the menu bar, and a single Background
Work switch in Settings turns all of it off. Everything still works when
you ask directly; nothing runs on its own.

**Can my other tools see what Night Shift did?**
Yes. Results are ordinary notes and reports, so anything connected to
Alchemy's agent interface can read last night's work, and can leave work
for tonight, the same as you.

## Homepage cut

Replacement copy for the Night Shift utility card in `docs/index.html`
(currently "Let Night Shift run the routine work."), plus one new card if
the area ships. Register: card h3 is Apple-terse; body is one plain
sentence; chips are clipped fragments.

**Card: Night Shift (revised)**

> h3: Hand the slow work to the night.
> p: Leave a job before bed. Read the result over coffee.
> chips: `est. $0.00 · local` / `Finished 3:41 AM`

**Card: Standing orders (new)**

> h3: Ask once. It keeps watching.
> p: Watched pages and standing questions fire when something changes, not
> when you remember to check.
> chips: `When the 10-K drops` / `armed`

**Card: The record (new, or folds into the Night Shift card's footer)**

> h3: Every run leaves a receipt.
> p: What was read, what was written, what it cost, and proof that sealed
> notebooks stayed on your Mac.
> chips: `4 runs` / `1 sealed · local`

Boundary line, reused verbatim where any of these appear:

> Night Shift writes notes and reports. It will not act outward.
