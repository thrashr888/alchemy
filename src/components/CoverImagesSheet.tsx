import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { thumbMemory } from "@/lib/thumbCache";
import { FOLDER_TYPES } from "@/lib/sourceFacets";
import type { CoverScan, Source } from "@/lib/types";
import { cn, isWebUrl } from "@/lib/utils";
import { sourceIcon } from "@/lib/sourceIcon";
import { Button, Chip, EmptyState, Modal, ProgressBar, Spinner } from "./ui";
import { Image as ImageIcon } from "lucide-react";

/** Sources scanned per call: four page fetches in flight, so a row never
 *  waits behind more than a few seconds of someone else's slow server. */
const BATCH = 4;

/** Does this source want a cover? Mirrors `needs_cover` in
 *  src-tauri/src/commands/covers.rs: PDFs and local images draw their own
 *  thumbnail, folders are only parents. */
export function needsCover(s: Source): boolean {
  return (
    s.sourceType !== "pdf" &&
    s.sourceType !== "image" &&
    !FOLDER_TYPES.includes(s.sourceType) &&
    (s.imageUrl === "" || s.imageUrl === "-")
  );
}

type RowState = { status: "scanning" | "done"; scan?: CoverScan };

/** The pick for one row: an image URL, or "none" to leave it as it is. */
type Pick = string;

/** Fix the sources that have no cover, a notebook at a time. The scan only
 *  proposes — it reads what each source already holds and, for a web page,
 *  fetches the page once — and nothing is written until Apply, and then only
 *  the images chosen here. */
export function CoverImagesSheet({
  open,
  onClose,
  notebookId,
}: {
  open: boolean;
  onClose: () => void;
  notebookId: string;
}) {
  const sources = useStore((s) => s.sources);
  const pushToast = useStore((s) => s.pushToast);

  // The list is fixed when the sheet opens: picks must not shuffle under the
  // cursor as the store refreshes.
  const targets = useMemo(
    () =>
      open
        ? sources
            .filter(needsCover)
            // Web pages first: they are where the covers went missing.
            .sort((a, b) => Number(b.sourceType === "url") - Number(a.sourceType === "url"))
        : [],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [open, notebookId],
  );

  const [rows, setRows] = useState<Record<string, RowState>>({});
  const [picks, setPicks] = useState<Record<string, Pick>>({});
  const [broken, setBroken] = useState<Record<string, boolean>>({});
  const [scanning, setScanning] = useState(false);
  const [applying, setApplying] = useState(false);
  const [showEmpty, setShowEmpty] = useState(false);
  const stopRef = useRef(false);

  useEffect(() => {
    if (!open) return;
    stopRef.current = false;
    setRows({});
    setPicks({});
    setBroken({});
    setShowEmpty(false);
    if (targets.length === 0) return;
    let cancelled = false;
    setScanning(true);
    void (async () => {
      for (let i = 0; i < targets.length; i += BATCH) {
        if (cancelled || stopRef.current) break;
        const ids = targets.slice(i, i + BATCH).map((s) => s.id);
        setRows((r) => {
          const next = { ...r };
          for (const id of ids) next[id] = { status: "scanning" };
          return next;
        });
        let scans: CoverScan[] = [];
        try {
          scans = (await api.scanCoverImages(notebookId, ids, BATCH)).rows;
        } catch {
          /* the api layer reports it; the rows below say "scan failed" */
        }
        if (cancelled) return;
        const byId = new Map(scans.map((s) => [s.sourceId, s]));
        setRows((r) => {
          const next = { ...r };
          for (const id of ids) {
            const t = targets.find((s) => s.id === id)!;
            next[id] = {
              status: "done",
              scan: byId.get(id) ?? {
                sourceId: id,
                title: t.title,
                sourceType: t.sourceType,
                candidates: [],
                note: "Scan failed",
              },
            };
          }
          return next;
        });
      }
      if (!cancelled) setScanning(false);
    })();
    return () => {
      cancelled = true;
      setScanning(false);
    };
  }, [open, notebookId, targets]);

  const done = Object.values(rows).filter((r) => r.status === "done").length;
  const scanned = targets.flatMap((source) =>
    rows[source.id] ? [{ source, state: rows[source.id] }] : [],
  );
  const offered = scanned.filter(
    (r) =>
      r.state.status === "scanning" || (r.state.scan?.candidates ?? []).length > 0,
  );
  const empty = scanned.filter(
    (r) =>
      r.state.status === "done" && (r.state.scan?.candidates ?? []).length === 0,
  );
  const chosen = Object.entries(picks).filter(([, p]) => p !== "none");

  const pickFirstForAll = () =>
    setPicks((p) => {
      const next = { ...p };
      for (const [id, r] of Object.entries(rows)) {
        const first = (r.scan?.candidates ?? []).find((u) => !broken[u]);
        if (first && !next[id]) next[id] = first;
      }
      return next;
    });

  const apply = async () => {
    setApplying(true);
    try {
      const report = await api.applyCoverImages(
        notebookId,
        chosen.map(([sourceId, imageUrl]) => ({ sourceId, imageUrl })),
      );
      for (const [id] of chosen) thumbMemory.delete(id);
      if (useStore.getState().currentId === notebookId) {
        useStore.setState({ sources: await api.listSources(notebookId) });
      }
      const skipped = report.skipped.length;
      pushToast(
        "success",
        `${report.applied} cover${report.applied === 1 ? "" : "s"} set` +
          (skipped ? `, ${skipped} skipped` : ""),
      );
      onClose();
    } catch {
      /* surfaced by the api layer's toast path */
    } finally {
      setApplying(false);
    }
  };

  const total = targets.length;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Cover images"
      width="max-w-3xl"
      tall
      footer={
        <div className="flex w-full items-center gap-2">
          <Button
            variant="ghost"
            onClick={pickFirstForAll}
            disabled={
              applying || offered.every((r) => !r.state.scan?.candidates.length)
            }
            title="Choose each source's best candidate where you have not chosen yet"
          >
            Use first candidate for all
          </Button>
          <div className="ml-auto flex items-center gap-2">
            <span className="text-caption text-muted-foreground">
              {chosen.length} chosen
            </span>
            <Button variant="secondary" onClick={onClose} disabled={applying}>
              Cancel
            </Button>
            <Button
              variant="primary"
              onClick={() => void apply()}
              disabled={chosen.length === 0}
              loading={applying}
            >
              Apply
            </Button>
          </div>
        </div>
      }
    >
      {total === 0 ? (
        <EmptyState
          icon={<ImageIcon className="h-5 w-5" />}
          title="Every source has a cover"
          hint="PDFs and images draw their own thumbnail, so they are not listed."
        />
      ) : (
        <div className="flex flex-col gap-3">
          <p className="text-caption text-muted-foreground">
            {total} source{total === 1 ? "" : "s"} without a cover. Pick an
            image for the ones you want; nothing changes until Apply.
          </p>
          <div className="flex items-center gap-3">
            <ProgressBar
              className="flex-1"
              done={done}
              total={total}
              label="sources scanned"
            />
            <span className="shrink-0 text-caption tabular-nums text-muted-foreground">
              {scanning ? `Scanned ${done} of ${total}` : `Scanned ${done}`}
            </span>
            {scanning && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  stopRef.current = true;
                  setScanning(false);
                }}
              >
                Stop
              </Button>
            )}
          </div>

          {offered.length > 0 && (
            <ul className="select-none divide-y divide-border overflow-hidden rounded-lg border border-border">
              {offered.map(({ source, state }) =>
                state.status === "scanning" ? (
                  <ScanningRow key={source.id} source={source} />
                ) : (
                  <ScanRow
                    key={source.id}
                    source={source}
                    scan={state.scan!}
                    pick={picks[source.id]}
                    broken={broken}
                    onBroken={(u) => setBroken((b) => ({ ...b, [u]: true }))}
                    onPick={(p) =>
                      setPicks((m) => {
                        const next = { ...m };
                        if (p === undefined) delete next[source.id];
                        else next[source.id] = p;
                        return next;
                      })
                    }
                  />
                ),
              )}
            </ul>
          )}

          {empty.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <button
                type="button"
                onClick={() => setShowEmpty((v) => !v)}
                aria-expanded={showEmpty}
                className="self-start rounded-md text-caption text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                {empty.length} without candidates {showEmpty ? "(hide)" : "(show)"}
              </button>
              {showEmpty && (
                <ul className="divide-y divide-border overflow-hidden rounded-lg border border-border">
                  {empty.map(({ source, state }) => (
                    <li
                      key={source.id}
                      className="flex items-center gap-2 px-3 py-2 text-caption"
                    >
                      <span className="truncate text-foreground">
                        {source.title}
                      </span>
                      <span className="ml-auto shrink-0 text-subtle-foreground">
                        {state.scan?.note || "None in content"}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      )}
    </Modal>
  );
}

function ScanningRow({ source }: { source: Source }) {
  return (
    <li className="flex items-center gap-2 px-3 py-2.5">
      <Spinner className="h-3.5 w-3.5" />
      <span className="truncate text-body text-muted-foreground">
        {source.title}
      </span>
      <span className="ml-auto shrink-0 text-caption text-subtle-foreground">
        {isWebUrl(source.url) ? "Reading the page…" : "Reading…"}
      </span>
    </li>
  );
}

function ScanRow({
  source,
  scan,
  pick,
  broken,
  onBroken,
  onPick,
}: {
  source: Source;
  scan: CoverScan;
  pick: Pick | undefined;
  broken: Record<string, boolean>;
  onBroken: (url: string) => void;
  onPick: (pick: Pick | undefined) => void;
}) {
  const visible = scan.candidates.filter((u) => !broken[u]);
  return (
    <li className="flex flex-col gap-2 px-3 py-2.5">
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-body font-medium text-foreground">
            {source.title}
          </div>
          <div className="mt-0.5 flex min-w-0 items-center gap-1.5 text-caption text-subtle-foreground">
            {sourceIcon(source.sourceType, source.url)}
            <span className="truncate">
              {scan.note || (isWebUrl(source.url) ? source.url : source.sourceType)}
            </span>
          </div>
        </div>
        <Chip
          size="xs"
          active={pick === "none"}
          onClick={() => onPick(pick === "none" ? undefined : "none")}
          title="Leave this source without a cover"
        >
          None
        </Chip>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {visible.map((u) => (
          <button
            key={u}
            type="button"
            title={u}
            aria-pressed={pick === u}
            onClick={() => onPick(pick === u ? undefined : u)}
            className={cn(
              "h-14 w-20 overflow-hidden rounded-md bg-surface-2 outline-none transition-shadow",
              "focus-visible:ring-2 focus-visible:ring-ring/60",
              pick === u
                ? "ring-2 ring-primary"
                : "shadow-[inset_0_0_0_0.5px_var(--border)] hover:shadow-[inset_0_0_0_0.5px_var(--border-strong)]",
            )}
          >
            <img
              src={u}
              alt=""
              loading="lazy"
              referrerPolicy="no-referrer"
              onError={() => onBroken(u)}
              className="h-full w-full object-cover"
            />
          </button>
        ))}
      </div>
    </li>
  );
}
