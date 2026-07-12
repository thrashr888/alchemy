import { useEffect, useState, type ReactNode } from "react";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Button, Input, Textarea, Modal, Spinner } from "./ui";
import { cn } from "@/lib/utils";
import type { MacCollection } from "@/lib/types";
import {
  Calendar,
  ChevronLeft,
  ClipboardPaste,
  Link2,
  ListChecks,
  NotebookText,
  Upload,
  FolderOpen,
} from "lucide-react";

/** The Mac provider tiles (backed by cider); id doubles as the IPC provider key. */
const MAC_PROVIDERS = [
  { id: "calendar", label: "Calendar", icon: Calendar },
  { id: "reminders", label: "Reminders", icon: ListChecks },
  { id: "notes", label: "Apple Notes", icon: NotebookText },
] as const;

type MacProvider = (typeof MAC_PROVIDERS)[number];

type Step = "hub" | "url" | "text" | "mac";

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
  const [pasteTitle, setPasteTitle] = useState("");
  const [pasteText, setPasteText] = useState("");
  const [provider, setProvider] = useState<MacProvider>(MAC_PROVIDERS[0]);
  const [collections, setCollections] = useState<MacCollection[] | null>(null);
  const [macError, setMacError] = useState<string | null>(null);

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
    setStep("mac");
    api
      .listMacCollections(p.id)
      .then(setCollections)
      .catch((e) => setMacError(e instanceof Error ? e.message : String(e)));
  }

  const back = (
    <button
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
    <Modal open={open} onClose={closeAddSource} title={titles[step]} width="max-w-lg">
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
            <Upload className={cn("h-5 w-5", draggingFiles ? "text-primary" : "text-muted-foreground")} />
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
            <Tile icon={<Link2 className="h-4 w-4" />} label="From URL" onClick={() => setStep("url")} />
            <Tile
              icon={<ClipboardPaste className="h-4 w-4" />}
              label="Paste text"
              onClick={() => setStep("text")}
            />
          </div>

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
              <code className="rounded bg-surface-2 px-1 py-0.5">brew install cider</code>
            </p>
          )}
        </div>
      )}

      {step === "url" && (
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            closeAddSource();
            await addUrl(url);
          }}
          className="flex flex-col gap-3"
        >
          {back}
          <Input
            autoFocus
            placeholder="https://example.com/article"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
          />
          <p className="text-[11px] leading-relaxed text-subtle-foreground">
            Google Docs, Sheets, and Slides links work too — share them as
            “Anyone with the link” first.
          </p>
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
            <Button type="submit" variant="primary" disabled={!pasteText.trim()}>
              Add source
            </Button>
          </div>
        </form>
      )}

      {step === "mac" && (
        <div className="flex flex-col gap-1">
          {back}
          {macError ? (
            <p className="px-2 py-4 text-[12px] text-destructive">{macError}</p>
          ) : collections === null ? (
            <div className="flex items-center justify-center py-8">
              <Spinner className="h-4 w-4 text-muted-foreground" />
            </div>
          ) : collections.length === 0 ? (
            <p className="px-2 py-4 text-[12px] text-muted-foreground">
              Nothing to add from {provider.label}.
            </p>
          ) : (
            collections.map((c) => (
              <button
                key={c.id}
                onClick={() => {
                  closeAddSource();
                  void addMac(provider.id, c.id, c.label);
                }}
                className="flex items-baseline gap-2 rounded-md px-2 py-2 text-left transition-colors hover:bg-surface-2 focus-visible:bg-surface-2 outline-none"
              >
                <span className="min-w-0 truncate text-[13px] text-foreground">{c.label}</span>
                <span className="ml-auto shrink-0 text-[11px] text-subtle-foreground">
                  {c.detail}
                </span>
              </button>
            ))
          )}
          <p className="mt-2 text-[11px] leading-relaxed text-subtle-foreground">
            Content from this item is embedded into this notebook's local index
            and re-syncs automatically.
          </p>
        </div>
      )}
    </Modal>
  );
}
