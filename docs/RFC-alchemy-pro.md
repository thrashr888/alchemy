# RFC: Alchemy Pro — what a paid tier would be, if there is one

Exploration, not a commitment. The question: if Alchemy grew a paid
tier, what would it contain, what would it cost, and what must it
never touch? Grounded in live competitor data (Raindrop Pro, Liminary,
mymind — pricing pages read 2026-08-02) and three independently
designed proposals (services lens, license lens, intelligence lens)
that turned out to converge on one architectural fact.

## 1. The landscape, as of today

| | Raindrop Pro | mymind | Liminary Pro |
|---|---|---|---|
| Price | $3/mo · $28/yr | $7.99–12.99/mo · $79–129/yr | $29/mo (Team $50) |
| Free tier | Unlimited capture, forever | None marketed (100-card guest) | 200 docs, hard caps |
| What's gated | Durability + retrieval: full-text search, permanent copies, backups, dedupe, AI assistant | Everything — paying *is* the product ("100% privacy" is a literal feature bullet) | Intelligence + caps: fact-check/gap detection, meeting sidekick, model choice, **MCP/agent access** |
| Enforcement | Server-side (their cloud) | Server-side (their cloud) | Server-side (their cloud) |
| Posture | "Simple pricing," one tier, collaboration explicitly free | Proudly indie, no VC, anti-feature manifesto, identity tier names | AI-priced SaaS, "your expertise compounds here" |

Three positions on one axis: Raindrop charges for *durability at
scale*, mymind charges for *privacy and calm as identity*, Liminary
charges for *the intelligence layer*. Alchemy's awkwardness — and its
edge — is that it already ships all three for free: full-text hybrid
search over everything, on-device privacy stronger than mymind can
claim from a cloud, and a deeper intelligence layer (deep research,
Ledger/Weave/Second Look, Night Shift, meta-chat) than Liminary sells
at $29/mo. Notably, Liminary gates MCP/agent access behind Pro;
Alchemy giving it away free is a quotable positioning edge worth
keeping forever.

## 2. The constraint that decides everything

Alchemy is a public repo with no accounts, no server, and no
telemetry. **Any client-side gate is a fiction — a fork strips it in a
weekend, and fighting that with DRM would betray the local-first
identity anyway.** So the only paywall that can exist is a server-side
artifact: an endpoint that answers, a credential that meters, a bucket
that stores. That single fact collapses the design space:

- **Gating existing on-device features** (license-key style) is
  unenforceable *and* violates the house rule — intelligent behavior
  ships default-ON; toggles are cost control, not gates. Rejected.
- **Usage caps on local data** (Liminary's 200-doc free tier) would be
  pure artificial scarcity — local retrieval has zero marginal cost.
  Rejected; unlimited local everything, forever.
- **What remains is exactly the honest paywall:** the things that put
  recurring charges on my bill. Sync relay storage. An always-on clip
  inbox. Metered inference for people with no provider. Server-side
  page archives. Every gated thing costs real money per user per
  month; everything with zero marginal cost stays free. The paywall
  *is* cost recovery — the same principle as the existing toggles,
  extended to my infrastructure instead of the user's.

This is also why the pieces are already in motion:
[RFC-sync-backend.md](RFC-sync-backend.md) designs the managed relay
(E2E ciphertext, self-hostable, doorman Worker + R2), prices it
(~$30–60/mo at 10k users), and leaves "Free quota and a paid tier" as
an open question with Obsidian's $4–8/mo named as the market rate.
Alchemy Pro is mostly that question, answered.

## 3. The free covenant

Written down so it can be pointed at, forever:

- **Everything that runs on your Mac is free, unlimited, forever.**
  All imports, grounded chat, hybrid retrieval at any corpus size, all
  19 generators and artifact renderers, Audio Overview, deep research,
  the whole Steward (Night Shift, Brief, Ledger, Weave, Second Look),
  reader, notes, gallery, themes. Nothing that ever shipped free moves
  behind the paywall — retroactive gating turns "default-ON" into a
  lie once and the brand never recovers.
- **BYO inference in full**, including deep rerank and global
  map-reduce when your own gateway pays for it.
- **The MCP server and every agent connector.** Liminary charges $29/mo
  for this; we don't, and we say so.
- **Data portability** — OKF export/share/import, `.alchemy` archives,
  plain local storage. You never pay to get your own corpus back.
- **Folder-transport sync** (iCloud Drive / Dropbox / Syncthing
  carrying the encrypted op log, per RFC-sync-backend §4d) — real
  multi-device sync with no account and no service, free.
- **The relay source.** The Worker ships in this repo; self-hosting is
  a supported deployment, not a fork. Pro is convenience, not
  capability.
- **Joining a shared notebook.** Raindrop's lesson: sharing drives
  acquisition, so receiving is always free. Hosting a share needs Pro.

## 4. What Pro contains

One tier, one decision — Raindrop's "Simple pricing" posture is right
for a solo-run service. In shipping order:

1. **Managed sync** (RFC-sync-backend phase 3) — the one-switch relay:
   notebooks follow you to every Mac, E2E-encrypted, zero setup on a
   second Mac via iCloud Keychain. The anchor feature; storage and
   Class-A ops are the recurring bill.
2. **Encrypted off-device backup** — nightly ciphertext snapshot of
   canonical data with point-in-time restore. Rides sync's plumbing
   almost free; answers "my laptop died." (Raindrop's shape exactly:
   manual export free, automated backup paid.)
3. **Shared notebooks** (phase 4) — hosting a share requires Pro;
   joining never does.
4. **Clip Inbox** — the web clipper today only talks to `127.0.0.1`
   while the app is awake. Pro adds a hosted, encrypted inbox: clip
   from your phone or a work machine, it lands when your Mac wakes.
   An always-on per-user endpoint is precisely a real cost.
5. **Alchemy Intelligence — bundled metered model credits.**
   RFC-inference-providers' own finding: most prospective users have
   no provider installed, not even Ollama. A hosted credential makes
   chat/OCR/rerank just work — and flips deep rerank + global
   map-reduce on for these users, which isn't un-gating a feature,
   it's me absorbing the gateway bill the default-ON logic already
   expects someone to pay. "Same brain, better fuel." Embeddings stay
   on-device regardless; the privacy line moves zero inches. Hard
   credit stops with graceful overflow to the user's own keys — the
   app never stops working at zero credits.
6. **iOS companion** (later; sync is the prerequisite) — capture and
   ask on the phone, corpus on the Mac. The premium surface
   self-hosters can't replicate, and the reason the bundle survives
   relay self-hosting. Free app with web-sold subscription,
   Raindrop-style, so Apple's cut applies to zero revenue.
7. **Pro is agent-reachable** — sync status, inbox drain, credit
   balance, archive retrieval as MCP tools. House rule, applied to the
   paid tier.

**Considered, held back: Permanent Web Archive** (Raindrop's most-loved
gate). Server-side capture sees page content in plaintext — the one
feature that breaks "the relay holds only ciphertext" — plus DMCA
exposure for a solo operator. If it ever ships it's opt-in per source
and loudly labeled; more likely the local rendered-page capture is
already 90% of the value with none of the liability.

## 5. Price

**$6/mo or $60/yr**, including 10 GB ciphertext storage and a monthly
model-credit allowance; credit top-up packs for heavy hosted-inference
use; self-hosted relay free forever.

Why that number: the sync RFC names the band ($4–8, Obsidian's price
for exactly this service). Raindrop's $3 proves storage-durability
alone can't price higher — but the bundle carries sync + inbox +
credits + (eventually) iOS, which Raindrop doesn't. mymind's
$7.99–12.99 brackets the indie-prosumer ceiling; Liminary's $29 is
what cloud-everything AI costs when the vendor pays for all inference,
which we don't. The hybrid structure is load-bearing: flat sub covers
predictable costs (storage, relay, inbox), metering caps the one
unpredictable cost (tokens). Never promise unlimited inference on a
$6 sub.

Billing direct via Paddle or Lemon Squeezy as merchant of record (VAT,
refunds, PayPal — no App Store rails exist and the Mac App Store would
gut the MCP server, cider, and Services integration anyway). 30-day
money-back, graceful downgrade: sync pauses, nothing is lost, the app
keeps working entirely — worst-case failure of the paid tier is that
Alchemy becomes exactly the free product, which still works forever.

**A patronage skin, not a second SKU:** a Founding Member yearly
option (~$79, mymind's Student-of-Life price) — same features,
About-box credit, the "proudly indie, you fund the maintainer instead
of ads" identity purchase mymind proved people want. One SKU with a
generous checkbox beats a tier ladder.

The RFC's rule stands: **no tier before real cost data.** Sync beta
runs free with quotas; the $6 is an anchor to test against actual R2
and token bills, not a commitment.

## 6. What was explicitly rejected

- **A one-time $99 license gating future features** (voice suite,
  remaining V12 steward pillars, pro exports). Tempting — "own your
  tools" rhymes with "own your data" — but it's unenforceable in a
  public repo, one-time revenue can't fund recurring infrastructure,
  and the free/Pro seam inside the Steward ("the text brief is free
  but the spoken one is $99?") reads as arbitrary. Voice, watchers,
  the Tickler, and Brief audio all ship free when they ship: they run
  on the user's Mac and cost me nothing.
- **Gating intelligence quality.** The first retrieval or generation
  improvement that lands Pro-only ends the "default-ON" story
  permanently. Pro buys fuel and infrastructure, never a smarter
  brain.
- **Seats, SSO, enterprise anything.** V12's boundary holds: single
  user, explicitly not enterprise. A work team that wants custody
  self-hosts the relay — that's the enterprise story, and it's free.

## 7. Risks

- **Solo operator of a paid service**: uptime expectations, support
  load, durability promises. Mitigated by the dumb-ciphertext design —
  worst outage pauses sync, never breaks the app — but "the backup
  service lost my backup" is existential; write a durability statement
  and replicate before charging.
- **Version skew becomes a paying-customer fire**: in-place
  schema-append migrations brick older binaries (known gotcha); two
  synced Macs on different versions need schema-version negotiation
  before GA.
- **Token-cost exposure**: one enthusiastic overnight-research user can
  burn past $6. Hard stops and visible overflow-to-own-keys, or the
  ethos ("usage is whatever your gateway bills") curdles.
- **Buyer mismatch**: current users (GitHub releases, Homebrew, own
  Claude subscriptions) are exactly the people who need hosted
  credits least. The credit buyer is non-technical and unreached by
  current channels; sync is the feature that converts the existing
  audience.
- **Self-hosting cannibalization**: accepted on purpose — it's the
  trust story. The iOS companion and credits are what self-hosters
  still buy.

## 8. Open questions

- Free sync quota (the RFC's 1 GB ciphertext?) and whether a verified
  email is required for free-tier sybil resistance.
- Credit allowance size — needs real token telemetry from the metering
  proxy before naming a number.
- Whether the Clip Inbox is v1-Pro or ships later; it's the only Pro
  feature besides sync that needs new server surface.
- Whether Founding Member is worth the SKU complexity at all, or the
  About-box credit just attaches to yearly billing.
- When (if ever) the Web Archive's value clears its plaintext/DMCA
  liability for a solo operator.
