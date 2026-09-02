import { describe, expect, it } from "vitest";
import {
  isStale,
  liveKind,
  parseCalendar,
  parseFeed,
  parseLiveCard,
  parseReminders,
  parseStocks,
} from "./liveCards";

// Fixtures are byte-for-byte what src-tauri/src/mac.rs renders (its tests
// pin the same strings), so a renderer drift fails on both sides.

const STOCKS = `# Stocks — My Symbols

| Symbol | Name | Price | Change | Status |
|---|---|---|---|---|
| AAPL | Apple Inc. | 231.46 USD | -1.23% | closed |
| ZZZZ |  |  |  |  |

_Prices as of 2026-09-02T20:00:00Z (Apple Stocks cache)._
`;

const CALENDAR = `# Calendar — next 7 days

## 2026-09-03
- 14:00 — Inspection (Home) at 12 Elm St
  - bring the report
## 2026-09-07
- all day — Labor Day (US Holidays)
`;

const REMINDERS = `# Reminders — Home

- [ ] Call the insurer \`x-apple-reminder://A1\` — due 2026-09-04
  - policy 4471
- [ ] Buy stamps \`x-apple-reminder://B2\`
`;

const FEED = `# Tauri blog

Release notes and announcements.

- 2026-09-01T10:00 — [Tauri 2.9](https://tauri.app/blog/tauri-2-9) — Smaller bundles and a new tray API.
- 2026-08-20 — Community roundup
`;

describe("liveKind", () => {
  it("names the card for live sources and nothing else", () => {
    expect(liveKind({ sourceType: "mac", url: "cider://stocks/watchlist/My Symbols" })).toBe("stocks");
    expect(liveKind({ sourceType: "mac", url: "cider://calendar/upcoming/7" })).toBe("calendar");
    expect(liveKind({ sourceType: "mac", url: "cider://reminders/list/Home" })).toBe("reminders");
    expect(liveKind({ sourceType: "feed", url: "https://tauri.app/feed.xml" })).toBe("feed");
    // An Apple Note is prose already; a page is not live.
    expect(liveKind({ sourceType: "mac", url: "cider://notes/note/x" })).toBeNull();
    expect(liveKind({ sourceType: "url", url: "https://tauri.app" })).toBeNull();
  });
});

describe("parseStocks", () => {
  it("reads the quote table and the as-of stamp", () => {
    const card = parseStocks(STOCKS);
    expect(card?.asOf).toBe("2026-09-02T20:00:00Z");
    expect(card?.rows).toEqual([
      { symbol: "AAPL", name: "Apple Inc.", price: "231.46 USD", change: "-1.23%", changePct: -1.23, status: "closed" },
      { symbol: "ZZZZ", name: "", price: "", change: "", changePct: null, status: "" },
    ]);
  });
  it("falls back to nothing on corrupted text", () => {
    expect(parseStocks("# Stocks — My Symbols\n\nnothing here")).toBeNull();
    expect(parseStocks(STOCKS.replace("| Symbol |", "| Ticker |"))).toBeNull();
    // A row with the wrong column count means the shape drifted.
    expect(parseStocks(STOCKS.replace("| closed |", "|"))).toBeNull();
  });
});

describe("parseCalendar", () => {
  it("reads day headings, times, calendars, locations and notes", () => {
    expect(parseCalendar(CALENDAR)?.events).toEqual([
      { day: "2026-09-03", time: "14:00", title: "Inspection", calendar: "Home", location: "12 Elm St", notes: "bring the report" },
      { day: "2026-09-07", time: "all day", title: "Labor Day", calendar: "US Holidays", location: null, notes: null },
    ]);
  });
  it("keeps a parenthesised title whole", () => {
    const card = parseCalendar("## 2026-09-03\n- 09:00 — Standup (maybe) (Work)\n");
    expect(card?.events[0]).toMatchObject({ title: "Standup (maybe)", calendar: "Work" });
  });
  it("falls back to nothing without a day heading or on prose", () => {
    expect(parseCalendar("- 09:00 — Standup (Work)\n")).toBeNull();
    expect(parseCalendar("# Calendar\n\nNo events.\n")).toBeNull();
  });
});

describe("parseReminders", () => {
  it("reads title, id, due and notes", () => {
    expect(parseReminders(REMINDERS)?.items).toEqual([
      { title: "Call the insurer", id: "x-apple-reminder://A1", due: "2026-09-04", done: false, notes: "policy 4471" },
      { title: "Buy stamps", id: "x-apple-reminder://B2", due: null, done: false, notes: null },
    ]);
  });
  it("reads a checked box as done", () => {
    expect(parseReminders("- [x] Paid `r1`\n")?.items[0].done).toBe(true);
  });
  it("falls back to nothing on prose", () => {
    expect(parseReminders("# Reminders — Home\n\nAll clear.\n")).toBeNull();
  });
});

describe("parseFeed", () => {
  it("reads linked and bare entries", () => {
    expect(parseFeed(FEED)?.entries).toEqual([
      { published: "2026-09-01T10:00", title: "Tauri 2.9", link: "https://tauri.app/blog/tauri-2-9", excerpt: "Smaller bundles and a new tray API." },
      { published: "2026-08-20", title: "Community roundup", link: null, excerpt: null },
    ]);
  });
  it("falls back to nothing on prose", () => {
    expect(parseFeed("# Tauri blog\n\nRelease notes.\n")).toBeNull();
  });
});

describe("parseLiveCard / isStale", () => {
  it("dispatches by kind", () => {
    expect(parseLiveCard("stocks", STOCKS)?.kind).toBe("stocks");
    expect(parseLiveCard("reminders", STOCKS)).toBeNull();
  });
  it("calls a day-old copy stale and a fresh one not", () => {
    const now = 1_000_000_000_000;
    expect(isStale(now - 60_000, now)).toBe(false);
    expect(isStale(now - 25 * 3_600_000, now)).toBe(true);
    // No stamp at all (legacy rows) is unknown, not stale.
    expect(isStale(0, now)).toBe(false);
  });
});
