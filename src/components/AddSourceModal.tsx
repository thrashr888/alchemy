import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, Input, Textarea, Modal, Spinner } from "./ui";
import { cn } from "@/lib/utils";
import type { MacCollection } from "@/lib/types";
import { FdaHint } from "./MacConnect";
import {
  Calendar,
  ChevronLeft,
  ChevronRight,
  ClipboardPaste,
  Folder,
  Link2,
  ListChecks,
  NotebookText,
  TrendingUp,
  Upload,
  FolderOpen,
} from "lucide-react";

/** The Mac provider tiles (backed by cider); id doubles as the IPC provider key. */
const MAC_PROVIDERS = [
  { id: "calendar", label: "Calendar", icon: Calendar },
  { id: "reminders", label: "Reminders", icon: ListChecks },
  { id: "notes", label: "Apple Notes", icon: NotebookText },
  { id: "stocks", label: "Stocks", icon: TrendingUp },
] as const;

type MacProvider = (typeof MAC_PROVIDERS)[number];

type Step = "hub" | "url" | "text" | "mac";

/** Client-side mirror of the backend's git URL grammar (RFC-git-sources §1)
 *  — just enough shape detection to show the include ladder before import.
 *  Conservative on unknown hosts: only unambiguous shapes (clone URLs,
 *  /tree, /blob) light up; the backend's host probe decides the rest. */
type GitShape = "repo" | "tree" | "blob" | "clone" | null;
function gitShape(raw: string): GitShape {
  const u = raw.trim().replace(/\/+$/, "");
  if (/^git@[^/]+:/.test(u) || u.startsWith("ssh://") || /\.git$/.test(u))
    return "clone";
  const m = u.match(/^https?:\/\/(?:www\.)?([^/?#]+)\/([^/?#]+)\/([^/?#]+)(?:\/(.*))?$/);
  if (!m) return null;
  const [, host, owner, , rest] = m;
  if (!host.includes(".")) return null;
  const reserved = [
    "orgs", "organizations", "settings", "marketplace", "topics", "search",
    "login", "features", "about", "pricing", "explore", "sponsors",
    "notifications", "issues", "pulls", "collections", "events", "trending",
  ];
  if (reserved.includes(owner.toLowerCase())) return null;
  if (rest?.startsWith("tree/")) return "tree";
  if (rest?.startsWith("blob/")) return "blob";
  if (rest) return null;
  return host === "github.com" ? "repo" : null;
}

/** Ladder rungs offered per shape; first entry is the shape's default. */
function includeOptions(shape: GitShape) {
  if (shape === "repo")
    return [
      { v: "readme", label: "README" },
      { v: "docs", label: "Docs" },
      { v: "full", label: "Everything" },
    ];
  if (shape === "tree" || shape === "clone")
    return [
      { v: "full", label: "Everything" },
      { v: "docs", label: "Docs" },
    ];
  return [];
}

/** One tile on the hub: icon over label, same visual weight as the old menu rows. */
function Tile({
  icon,
  label,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex flex-col items-center justify-center gap-1.5 rounded-md border border-border bg-surface-2/60 px-2 py-3",
        "text-[12px] text-foreground/90 transition-colors hover:border-border-strong hover:bg-surface-2 hover:text-foreground",
        "focus-visible:ring-2 focus-visible:ring-ring/60 outline-none",
      )}
    >
      <span className="text-muted-foreground">{icon}</span>
      {label}
    </button>
  );
}

/**
 * The single "add sources" surface (NotebookLM-style): a hub of tiles —
 * upload, folder, URL, paste, and the Mac providers when cider is installed —
 * with the URL/paste forms and the Mac collection picker as second steps.
 * Opened from the panel +, the collapsed rail, and the Cmd+K menu via
 * `openAddSource(step?)`; renders globally from Workspace so it works while
 * the Sources panel is collapsed.
 */
export function AddSourceModal() {
  const open = useStore((s) => s.addSourceOpen);
  const deepStep = useStore((s) => s.addSourceStep);
  const closeAddSource = useStore((s) => s.closeAddSource);
  const openAddSource = useStore((s) => s.openAddSource);
  const macAvailable = useStore((s) => s.macAvailable);
  const draggingFiles = useStore((s) => s.draggingFiles);
  const pickAndAddFiles = useStore((s) => s.pickAndAddFiles);
  const pickAndAddFolder = useStore((s) => s.pickAndAddFolder);
  const addUrl = useStore((s) => s.addSourceUrl);
  const addText = useStore((s) => s.addSourceText);
  const addMac = useStore((s) => s.addSourceMac);

  const [step, setStep] = useState<Step>("hub");
  const [url, setUrl] = useState("");
  /** Include-ladder choice for git-shaped URLs; null = the shape's default. */
  const [include, setInclude] = useState<string | null>(null);
  const [pasteTitle, setPasteTitle] = useState("");
  const [pasteText, setPasteText] = useState("");
  const [provider, setProvider] = useState<MacProvider>(MAC_PROVIDERS[0]);
  const [collections, setCollections] = useState<MacCollection[] | null>(null);
  const [macError, setMacError] = useState<string | null>(null);
  const [macQuery, setMacQuery] = useState("");
  // Apple Notes drill-down: null = the folder list, a name = that folder's notes.
  const [notesFolder, setNotesFolder] = useState<string | null>(null);

  // Reset to a fresh hub (or the deep-linked form) every time the modal opens.
  useEffect(() => {
    if (!open) return;
    setStep(deepStep ?? "hub");
    setUrl("");
    setPasteTitle("");
    setPasteText("");
  }, [open, deepStep]);

  // "Add source from URL / paste text" asks come in as store flags rather
  // than events: this modal may still be mounting when the ask happens.
  const pendingAddUrl = useStore((s) => s.pendingAddUrl);
  const pendingAddText = useStore((s) => s.pendingAddText);
  useEffect(() => {
    if (pendingAddUrl) {
      useStore.setState({ pendingAddUrl: false });
      openAddSource("url");
    }
  }, [pendingAddUrl, openAddSource]);
  useEffect(() => {
    if (pendingAddText) {
      useStore.setState({ pendingAddText: false });
      openAddSource("text");
    }
  }, [pendingAddText, openAddSource]);

  function openMac(p: MacProvider) {
    setProvider(p);
    setCollections(null);
    setMacError(null);
    setMacQuery("");
    setNotesFolder(null);
    setStep("mac");
    api
      .listMacCollections(p.id)
      .then(setCollections)
      .catch((e) => setMacError(e instanceof Error ? e.message : String(e)));
  }

  const back = (
    <button
      // type matters: an untyped button is a SUBMIT button, so pressing
      // Enter in the URL input would "click" this and bounce to the hub.
      type="button"
      onClick={() => setStep("hub")}
      className="mb-3 flex items-center gap-1 text-[12px] text-muted-foreground transition-colors hover:text-foreground"
    >
      <ChevronLeft className="h-3.5 w-3.5" />
      All sources
    </button>
  );

  const titles: Record<Step, string> = {
    hub: "Add sources",
    url: "Add source from URL",
    text: "Paste text",
    mac: `Add from ${provider.label}`,
  };

  return (
    <Modal
      open={open}
      onClose={closeAddSource}
      title={titles[step]}
      width="max-w-lg"
    >
      {step === "hub" && (
        <div className="flex flex-col gap-3">
          {/* The OS drop already works anywhere on the window (FileDrop.tsx);
              this zone is the affordance, and a click browses instead. */}
          <button
            onClick={() => {
              closeAddSource();
              void pickAndAddFiles();
            }}
            className={cn(
              "flex flex-col items-center justify-center gap-1.5 rounded-lg border-2 border-dashed px-4 py-7",
              "transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
              draggingFiles
                ? "border-primary/60 bg-primary/10"
                : "border-border hover:border-border-strong hover:bg-surface-2/60",
            )}
          >
            <Upload
              className={cn(
                "h-5 w-5",
                draggingFiles ? "text-primary" : "text-muted-foreground",
              )}
            />
            <span className="text-[13px] font-medium text-foreground">
              Drop files or folders here
            </span>
            <span className="text-[11px] text-subtle-foreground">
              PDF · Office · images · text — or click to browse
            </span>
          </button>

          <div className="grid grid-cols-4 gap-2">
            <Tile
              icon={<Upload className="h-4 w-4" />}
              label="Upload files"
              onClick={() => {
                closeAddSource();
                void pickAndAddFiles();
              }}
            />
            <Tile
              icon={<FolderOpen className="h-4 w-4" />}
              label="Add folder"
              onClick={() => {
                closeAddSource();
                void pickAndAddFolder();
              }}
            />
            <Tile
              icon={<Link2 className="h-4 w-4" />}
              label="From URL"
              onClick={() => setStep("url")}
            />
            <Tile
              icon={<ClipboardPaste className="h-4 w-4" />}
              label="Paste text"
              onClick={() => setStep("text")}
            />
          </div>
          <p className="text-[11px] leading-relaxed text-subtle-foreground">
            URLs cover web pages, Google Docs, and GitHub or git repositories
            — repos import as living sources that re-sync automatically.
          </p>

          {macAvailable === true && (
            <div className="grid grid-cols-4 gap-2">
              {MAC_PROVIDERS.map((p) => (
                <Tile
                  key={p.id}
                  icon={<p.icon className="h-4 w-4" />}
                  label={p.label}
                  onClick={() => openMac(p)}
                />
              ))}
            </div>
          )}
          {macAvailable === false && (
            <p className="text-[11px] leading-relaxed text-subtle-foreground">
              Connect Calendar, Reminders & Notes —{" "}
              <code className="rounded bg-surface-2 px-1 py-0.5">
                brew install cider
              </code>
            </p>
          )}
        </div>
      )}

      {step === "url" && (
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            closeAddSource();
            const shape = gitShape(url);
            const opts = includeOptions(shape);
            await addUrl(
              url,
              opts.length > 0 ? (include ?? opts[0].v) : undefined,
            );
          }}
          className="flex flex-col gap-3"
        >
          {back}
          <Input
            autoFocus
            placeholder="https://example.com/article"
            value={url}
            onChange={(e) => {
              setUrl(e.target.value);
              setInclude(null);
            }}
          />
          {includeOptions(gitShape(url)).length > 0 ? (
            <div className="flex flex-col gap-1.5">
              <div className="flex items-center gap-1.5">
                <span className="text-[11px] font-medium uppercase tracking-wide text-subtle-foreground">
                  Import
                </span>
                {includeOptions(gitShape(url)).map((o) => {
                  const active = (include ?? includeOptions(gitShape(url))[0].v) === o.v;
                  return (
                    <button
                      key={o.v}
                      type="button"
                      onClick={() => setInclude(o.v)}
                      className={
                        active
                          ? "rounded-full bg-primary/15 px-2.5 py-0.5 text-[12px] text-citation"
                          : "rounded-full px-2.5 py-0.5 text-[12px] text-muted-foreground hover:bg-surface-2"
                      }
                    >
                      {o.label}
                    </button>
                  );
                })}
              </div>
              <p className="text-[11px] leading-relaxed text-subtle-foreground">
                A git repository — import just the README, prose docs, or docs
                and code. Re-syncs automatically with your own git
                credentials; widen it later from the source's Refresh.
              </p>
            </div>
          ) : (
            <p className="text-[11px] leading-relaxed text-subtle-foreground">
              Google Docs, Sheets, and Slides links work too — share them as
              “Anyone with the link” first.
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={closeAddSource}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={!url.trim()}>
              Fetch & add
            </Button>
          </div>
        </form>
      )}

      {step === "text" && (
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            closeAddSource();
            await addText(pasteTitle, pasteText);
          }}
          className="flex flex-col gap-3"
        >
          {back}
          <Input
            autoFocus
            placeholder="Title (optional)"
            value={pasteTitle}
            onChange={(e) => setPasteTitle(e.target.value)}
          />
          <Textarea
            rows={10}
            placeholder="Paste or type your text here…"
            value={pasteText}
            onChange={(e) => setPasteText(e.target.value)}
          />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={closeAddSource}>
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              disabled={!pasteText.trim()}
            >
              Add source
            </Button>
          </div>
        </form>
      )}

      {step === "mac" && (
        <MacPicker
          provider={provider}
          collections={collections}
          error={macError}
          query={macQuery}
          setQuery={setMacQuery}
          notesFolder={notesFolder}
          setNotesFolder={setNotesFolder}
          back={back}
          onPick={(c) => {
            closeAddSource();
            void addMac(provider.id, c.id, c.label);
          }}
        />
      )}
    </Modal>
  );
}

/**
 * The Mac collection picker: an auto-focused search over everything the
 * provider returned, and — for Apple Notes, where "everything" is the whole
 * library — a folder list to drill into instead of one long flat list.
 * Searching cuts across all folders; clearing the query returns to where
 * you were.
 */
function MacPicker({
  provider,
  collections,
  error,
  query,
  setQuery,
  notesFolder,
  setNotesFolder,
  back,
  onPick,
}: {
  provider: MacProvider;
  collections: MacCollection[] | null;
  error: string | null;
  query: string;
  setQuery: (q: string) => void;
  notesFolder: string | null;
  setNotesFolder: (f: string | null) => void;
  back: ReactNode;
  onPick: (c: MacCollection) => void;
}) {
  const searchable = provider.id !== "calendar";
  const q = query.trim().toLowerCase();

  // Apple Notes folders, derived from each note's folder name (its detail).
  const folders = useMemo(() => {
    if (provider.id !== "notes" || !collections) return [];
    const counts = new Map<string, number>();
    for (const c of collections)
      counts.set(c.detail, (counts.get(c.detail) ?? 0) + 1);
    return [...counts.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [provider.id, collections]);

  const showFolders = provider.id === "notes" && !q && notesFolder === null;
  const visible = (collections ?? []).filter((c) => {
    if (q)
      return (
        c.label.toLowerCase().includes(q) || c.detail.toLowerCase().includes(q)
      );
    if (provider.id === "notes") return c.detail === notesFolder;
    return true;
  });
  // The folder is redundant on every row while inside that folder.
  const showDetail = provider.id !== "notes" || notesFolder === null || !!q;

  return (
    <div className="flex flex-col gap-1">
      {back}
      {searchable && (
        <Input
          autoFocus
          placeholder={`Search ${provider.label}…`}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="mb-1"
        />
      )}
      {error ? (
        error.includes("Full Disk Access") ? (
          <div className="py-2">
            <FdaHint message={error} />
          </div>
        ) : (
          <p className="px-2 py-4 text-[12px] text-destructive [overflow-wrap:anywhere]">{error}</p>
        )
      ) : collections === null ? (
        <div className="flex items-center justify-center py-8">
          <Spinner className="h-4 w-4 text-muted-foreground" />
        </div>
      ) : collections.length === 0 ? (
        <p className="px-2 py-4 text-[12px] text-muted-foreground">
          Nothing to add from {provider.label}.
        </p>
      ) : (
        <div className="flex max-h-72 flex-col gap-0.5 overflow-y-auto">
          {provider.id === "notes" && !q && notesFolder !== null && (
            <button
              onClick={() => setNotesFolder(null)}
              className="flex items-center gap-1 rounded-md px-2 py-1.5 text-left text-[12px] text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground outline-none focus-visible:bg-surface-2"
            >
              <ChevronLeft className="h-3.5 w-3.5" />
              {notesFolder}
            </button>
          )}
          {showFolders
            ? folders.map(([name, count]) => (
                <button
                  key={name}
                  onClick={() => setNotesFolder(name)}
                  className="flex items-center gap-2 rounded-md px-2 py-2 text-left transition-colors hover:bg-surface-2 focus-visible:bg-surface-2 outline-none"
                >
                  <Folder className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <span className="min-w-0 truncate text-[13px] text-foreground">
                    {name}
                  </span>
                  <span className="ml-auto shrink-0 text-[11px] text-subtle-foreground">
                    {count} {count === 1 ? "note" : "notes"}
                  </span>
                  <ChevronRight className="h-3.5 w-3.5 shrink-0 text-subtle-foreground" />
                </button>
              ))
            : visible.map((c) => (
                <button
                  key={c.id}
                  onClick={() => onPick(c)}
                  className="flex items-baseline gap-2 rounded-md px-2 py-2 text-left transition-colors hover:bg-surface-2 focus-visible:bg-surface-2 outline-none"
                >
                  <span className="min-w-0 truncate text-[13px] text-foreground">
                    {c.label}
                  </span>
                  {showDetail && (
                    <span className="ml-auto shrink-0 text-[11px] text-subtle-foreground">
                      {c.detail}
                    </span>
                  )}
                </button>
              ))}
          {!showFolders && visible.length === 0 && q && (
            <p className="px-2 py-4 text-[12px] text-muted-foreground">
              No matches for “{query.trim()}”.
            </p>
          )}
        </div>
      )}
      <p className="mt-2 text-[11px] leading-relaxed text-subtle-foreground">
        {provider.id === "notes"
          ? "The note's full text becomes one source and re-syncs as you edit it. "
          : provider.id === "reminders"
            ? "A list syncs as one source — new reminders are picked up automatically. "
            : provider.id === "stocks"
              ? "A watchlist syncs as one source with the latest prices from the Stocks app. "
              : ""}
        Content is embedded into this notebook's local index and re-syncs
        automatically.
      </p>
    </div>
  );
}
