/**
 * Live answer cards (docs/RFC-events.md §7): when a chat reply cites a live
 * source, the reply grows a native card beneath the prose, rendered from the
 * source's STORED text — never from a model. These are the parsers, in the
 * MindMap/Flashcards contract: a rigid spec goes in, a typed card comes out,
 * and anything that does not parse is `null` (the prose stands alone).
 *
 * The specs are what `src-tauri/src/mac.rs` renders (its tests pin them —
 * change a renderer only together with its parser here):
 *
 *   stocks     `| Symbol | Name | Price | Change | Status |` rows, then
 *              `_Prices as of <time> (Apple Stocks cache)._`
 *   calendar   `## YYYY-MM-DD` day headings, `- HH:MM — Title (Calendar)
 *              [ at Location]` per event, `all day` in the time slot
 *   reminders  `- [ ] Title \`id\`[ — due YYYY-MM-DD]`, notes as `  - …`
 *   feed       one entry per line: `- YYYY-MM-DD[THH:MM] — [Title](link)
 *              [ — excerpt]` (the feed parent's rolling index, RFC §2; a
 *              plain `Title` without a link is accepted too)
 */

export type LiveKind = "stocks" | "calendar" | "reminders" | "feed";

/** Which card a source draws, or null when it is not live (or is a live
 *  source with no tabular shape — an Apple Note is prose already). */
export function liveKind(source: { sourceType: string; url: string }): LiveKind | null {
  if (source.sourceType === "feed") return "feed";
  if (source.sourceType !== "mac") return null;
  if (source.url.startsWith("cider://stocks/")) return "stocks";
  if (source.url.startsWith("cider://calendar/")) return "calendar";
  if (source.url.startsWith("cider://reminders/")) return "reminders";
  return null;
}

export interface StockRow {
  symbol: string;
  name: string;
  price: string;
  /** As rendered ("+1.23%"); empty when the quote cache had no row. */
  change: string;
  /** Signed percent parsed from `change`; null when blank or unparseable. */
  changePct: number | null;
  status: string;
}
export interface StocksCard {
  kind: "stocks";
  rows: StockRow[];
  /** The renderer's "as of" stamp, verbatim; null when no quote carried one. */
  asOf: string | null;
}

export interface CalendarItem {
  day: string;
  /** "HH:MM" or "all day". */
  time: string;
  title: string;
  calendar: string;
  location: string | null;
  notes: string | null;
}
export interface CalendarCard {
  kind: "calendar";
  events: CalendarItem[];
}

export interface ReminderItem {
  title: string;
  id: string | null;
  /** YYYY-MM-DD or null. */
  due: string | null;
  done: boolean;
  notes: string | null;
}
export interface RemindersCard {
  kind: "reminders";
  items: ReminderItem[];
}

export interface FeedEntry {
  published: string;
  title: string;
  link: string | null;
  excerpt: string | null;
}
export interface FeedCard {
  kind: "feed";
  entries: FeedEntry[];
}

export type LiveCard = StocksCard | CalendarCard | RemindersCard | FeedCard;

const lines = (text: string) => text.split("\n").map((l) => l.replace(/\s+$/, ""));

export function parseStocks(text: string): StocksCard | null {
  const ls = lines(text);
  const head = ls.findIndex((l) => /^\|\s*Symbol\s*\|\s*Name\s*\|\s*Price\s*\|\s*Change\s*\|\s*Status\s*\|$/.test(l));
  if (head < 0) return null;
  const rows: StockRow[] = [];
  for (let i = head + 1; i < ls.length; i++) {
    const l = ls[i];
    if (!l.startsWith("|")) break;
    if (/^\|[-\s|]+\|$/.test(l)) continue; // the header separator
    const cells = l.split("|").slice(1, -1).map((c) => c.trim());
    if (cells.length !== 5 || !cells[0]) return null;
    const pct = /^([+-]?\d+(?:\.\d+)?)%$/.exec(cells[3]);
    rows.push({
      symbol: cells[0],
      name: cells[1],
      price: cells[2],
      change: cells[3],
      changePct: pct ? Number(pct[1]) : null,
      status: cells[4],
    });
  }
  if (rows.length === 0) return null;
  const asOf = /_Prices as of (.+?) \(Apple Stocks cache\)\._/.exec(text);
  return { kind: "stocks", rows, asOf: asOf ? asOf[1] : null };
}

export function parseCalendar(text: string): CalendarCard | null {
  const events: CalendarItem[] = [];
  let day = "";
  for (const l of lines(text)) {
    const h = /^## (\d{4}-\d{2}-\d{2})$/.exec(l);
    if (h) {
      day = h[1];
      continue;
    }
    const ev = /^- (all day|\d{2}:\d{2}) — (.+)$/.exec(l);
    if (ev) {
      // Title is greedy up to the LAST "(Calendar)"; a location follows
      // " at ". A title with its own parentheses still resolves, because
      // the calendar group cannot itself contain parentheses.
      const m = /^(.*) \(([^()]*)\)(?: at (.+))?$/.exec(ev[2]);
      if (!m || !day) return null;
      events.push({
        day,
        time: ev[1],
        title: m[1],
        calendar: m[2],
        location: m[3] ?? null,
        notes: null,
      });
      continue;
    }
    const note = /^ {2}- (.+)$/.exec(l);
    if (note && events.length) events[events.length - 1].notes = note[1];
  }
  return events.length ? { kind: "calendar", events } : null;
}

export function parseReminders(text: string): RemindersCard | null {
  const items: ReminderItem[] = [];
  for (const l of lines(text)) {
    const m = /^- \[([ xX])\] (.+)$/.exec(l);
    if (m) {
      let rest = m[2];
      let due: string | null = null;
      let id: string | null = null;
      const d = / — due (\d{4}-\d{2}-\d{2})$/.exec(rest);
      if (d) {
        due = d[1];
        rest = rest.slice(0, -d[0].length);
      }
      const i = / `([^`]+)`$/.exec(rest);
      if (i) {
        id = i[1];
        rest = rest.slice(0, -i[0].length);
      }
      if (!rest) return null;
      items.push({ title: rest, id, due, done: m[1] !== " ", notes: null });
      continue;
    }
    const note = /^ {2}- (.+)$/.exec(l);
    if (note && items.length) items[items.length - 1].notes = note[1];
  }
  return items.length ? { kind: "reminders", items } : null;
}

export function parseFeed(text: string): FeedCard | null {
  const entries: FeedEntry[] = [];
  for (const l of lines(text)) {
    const m = /^- (\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2})?) — (.+)$/.exec(l);
    if (!m) continue;
    const linked = /^\[(.+?)\]\((\S+?)\)(?: — (.+))?$/.exec(m[2]);
    if (linked) {
      entries.push({ published: m[1], title: linked[1], link: linked[2], excerpt: linked[3] ?? null });
      continue;
    }
    const cut = m[2].indexOf(" — ");
    entries.push(
      cut < 0
        ? { published: m[1], title: m[2], link: null, excerpt: null }
        : { published: m[1], title: m[2].slice(0, cut), link: null, excerpt: m[2].slice(cut + 3) },
    );
  }
  return entries.length ? { kind: "feed", entries } : null;
}

/** One door: the card for a live source's stored text, or null. */
export function parseLiveCard(kind: LiveKind, text: string): LiveCard | null {
  switch (kind) {
    case "stocks":
      return parseStocks(text);
    case "calendar":
      return parseCalendar(text);
    case "reminders":
      return parseReminders(text);
    case "feed":
      return parseFeed(text);
  }
}

/** A card older than this says "as of …" instead of pretending. Mac sources
 *  resync every 15 minutes while the app runs, so a day-old copy means the
 *  app was closed or the Mac app was; a feed's own cadence tops out at a
 *  day (RFC §2). */
export const STALE_AFTER_MS = 24 * 60 * 60 * 1000;

export function isStale(fetchedAt: number, now = Date.now()): boolean {
  return fetchedAt > 0 && now - fetchedAt > STALE_AFTER_MS;
}
