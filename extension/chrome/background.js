// Alchemy Web Clipper: every action funnels into the app's alchemy://add
// deep link (see src-tauri/src/integrations.rs) — the extension holds no
// state, no credentials, and needs no host permissions.

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

chrome.action.onClicked.addListener((tab) => {
  if (!tab || !tab.id || !tab.url) return;
  send(tab.id, { url: tab.url, title: tab.title || "" });
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
    send(tab.id, { url: info.pageUrl, title: tab.title || "" });
  } else if (info.menuItemId === "alchemy-add-link") {
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
