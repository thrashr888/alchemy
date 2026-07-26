// Alchemy Web Clipper. Two paths funnel into the app's alchemy://add deep
// link (see src-tauri/src/integrations.rs):
//
//   - Links and selections: metadata-only, straight into the deep link.
//   - Whole pages: scrape the rendered DOM from THIS logged-in tab and POST
//     it to the app's local clip receiver (src-tauri/src/clip.rs) first, so
//     private / login-walled pages the app could never fetch itself still
//     ingest. The deep link fires either way; the app pairs the two by URL.
//
// The extension still holds no state and no credentials. It reads the active
// tab's DOM only on your click (activeTab), and only talks to 127.0.0.1.

// The app's clip receiver default port, plus the dev-build +1 offset — the
// extension can't read the app's discovery file, so it probes both.
const CLIP_PORTS = [41500, 41501];

function deepLink(params) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v) q.set(k, v);
  }
  return `alchemy://add?${q.toString()}`;
}

// Navigating the current tab to a custom protocol pops Chrome's
// "Open Alchemy.app?" confirmation without leaving the page.
function send(tabId, params) {
  chrome.tabs.update(tabId, { url: deepLink(params) });
}

// Runs IN the page (via scripting.executeScript). Mirrors capture.rs's
// EXTRACT_JS: the rendered outerHTML plus the bits readability can't recover
// (live title, OpenGraph title, byline/date) so the app's shared
// extracted_from_html path produces the same shape a webview rescue would.
function scrapePage() {
  const pick = (sel, attr) => {
    const el = document.querySelector(sel);
    if (!el) return "";
    return ((attr ? el.getAttribute(attr) : el.textContent) || "").trim();
  };
  let byline =
    pick('meta[name="author"]', "content") ||
    pick('meta[property="article:author"]', "content");
  let published =
    pick('meta[property="article:published_time"]', "content") ||
    pick('meta[name="date"]', "content") ||
    pick("time[datetime]", "datetime");
  const lds = Array.prototype.slice.call(
    document.querySelectorAll('script[type="application/ld+json"]'),
    0,
    5,
  );
  for (const s of lds) {
    if (byline && published) break;
    try {
      const d = JSON.parse(s.textContent);
      const nodes = Array.isArray(d) ? d : d["@graph"] || [d];
      for (const n of nodes) {
        if (!n || typeof n !== "object") continue;
        if (!published && n.datePublished) published = String(n.datePublished);
        const a = n.author;
        if (!byline && a) {
          byline = Array.isArray(a)
            ? a
                .map((x) => (x && x.name) || "")
                .filter(Boolean)
                .join(", ")
            : String((a && a.name) || "");
        }
      }
    } catch (e) {
      /* skip malformed JSON-LD */
    }
  }
  return {
    url: location.href,
    title: document.title || "",
    ogTitle: pick('meta[property="og:title"]', "content"),
    byline: byline || "",
    published: published || "",
    html: document.documentElement ? document.documentElement.outerHTML : "",
  };
}

// POST a scraped payload to whichever candidate port answers as the app.
// Resolves true on success; false if the receiver is off / unreachable, so
// the caller falls back to a URL-only clip.
async function postClip(payload) {
  const body = JSON.stringify(payload);
  for (const port of CLIP_PORTS) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/clip`, {
        method: "POST",
        // text/plain keeps this a CORS "simple request"; host_permissions
        // already exempt the fetch from CORS, so no preflight either way.
        headers: { "Content-Type": "text/plain" },
        body,
      });
      if (res.ok) return true;
    } catch (e) {
      /* try the next port */
    }
  }
  return false;
}

// Scrape the active tab and hand the DOM to the app, then fire the deep link.
// On any failure (non-web page, no scripting access, receiver off) fall back
// to the plain URL clip the app fetches itself.
async function clipPage(tab) {
  const fallback = () => send(tab.id, { url: tab.url, title: tab.title || "" });
  if (!/^https?:/i.test(tab.url || "")) return fallback();
  let payload;
  try {
    const [res] = await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: scrapePage,
    });
    payload = res && res.result;
  } catch (e) {
    return fallback();
  }
  if (!payload || !payload.html) return fallback();
  await postClip(payload);
  // Fire the deep link regardless: it summons the app and opens the add
  // modal. If the POST landed, ingest_url pairs it by URL; if not, the app
  // fetches the URL as before.
  send(tab.id, { url: payload.url, title: payload.title });
}

chrome.action.onClicked.addListener((tab) => {
  if (!tab || !tab.id || !tab.url) return;
  void clipPage(tab);
});

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "alchemy-add-page",
    title: "Add page to Alchemy",
    contexts: ["page"],
  });
  chrome.contextMenus.create({
    id: "alchemy-add-link",
    title: "Add link to Alchemy",
    contexts: ["link"],
  });
  chrome.contextMenus.create({
    id: "alchemy-add-selection",
    title: "Add selection to Alchemy",
    contexts: ["selection"],
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (!tab || !tab.id) return;
  if (info.menuItemId === "alchemy-add-page") {
    // Same rendered-DOM capture as the toolbar click.
    void clipPage(tab);
  } else if (info.menuItemId === "alchemy-add-link") {
    // A bare link isn't the open page — nothing to scrape; the app fetches it.
    send(tab.id, { url: info.linkUrl || "" });
  } else if (info.menuItemId === "alchemy-add-selection") {
    // Selection becomes a text source (the app prefers url over text, so
    // provenance rides inside the body rather than as a url param).
    const text = (info.selectionText || "").trim();
    send(tab.id, {
      text: info.pageUrl ? `${text}\n\nFrom: ${info.pageUrl}` : text,
      title: tab.title || "",
    });
  }
});
