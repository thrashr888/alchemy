import { useEffect, useState } from "react";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import { thumbMemory } from "@/lib/thumbCache";
import type { Source } from "@/lib/types";
import { cn } from "@/lib/utils";
import { Button, Input, Modal, Spinner } from "./ui";

/** Hand-pick a URL source's gallery card image: the ingest-time og:image
 *  pick misses some pages, so this offers the page's own images to choose
 *  from, plus a paste-a-URL escape hatch. "-" stores checked-none. */
export function SourceImagePicker({
  source,
  open,
  onClose,
}: {
  source: Source;
  open: boolean;
  onClose: () => void;
}) {
  const [candidates, setCandidates] = useState<string[] | null>(null);
  const [fetchFailed, setFetchFailed] = useState(false);
  const [manual, setManual] = useState("");
  const [saving, setSaving] = useState<string | null>(null);
  const [broken, setBroken] = useState<Record<string, boolean>>({});

  useEffect(() => {
    if (!open) return;
    setCandidates(null);
    setFetchFailed(false);
    setManual("");
    setBroken({});
    let stale = false;
    api
      .sourceImageCandidates(source.id)
      .then((urls) => {
        if (!stale) setCandidates(urls);
      })
      .catch(() => {
        if (!stale) {
          setCandidates([]);
          setFetchFailed(true);
        }
      });
    return () => {
      stale = true;
    };
  }, [open, source.id]);

  const choose = (url: string) => {
    setSaving(url);
    api
      .setSourceImage(source.id, url)
      .then((updated) => {
        useStore.setState((st) => ({
          sources: st.sources.map((s) =>
            s.id === updated.id ? { ...s, imageUrl: updated.imageUrl } : s,
          ),
        }));
        // The gallery's session cache holds the old visual — forget it so
        // the card re-resolves through the (also cleared) disk cache.
        thumbMemory.delete(source.id);
        useStore
          .getState()
          .pushToast(
            "success",
            url === "-" ? "Card image removed" : "Card image set",
          );
        onClose();
      })
      .catch(() => undefined) // surfaced by the api layer's toast path
      .finally(() => setSaving(null));
  };

  const visible = (candidates ?? []).filter((u) => !broken[u]);
  const manualTrimmed = manual.trim();
  const manualOk = /^https?:\/\//.test(manualTrimmed);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Card image"
      width="max-w-lg"
      footer={
        <div className="flex w-full items-center gap-2">
          {source.imageUrl !== "" && source.imageUrl !== "-" && (
            <Button
              variant="ghost"
              onClick={() => choose("-")}
              disabled={saving !== null}
              title="Show no image on this source's card"
            >
              No image
            </Button>
          )}
          <div className="ml-auto flex items-center gap-2">
            <Input
              value={manual}
              onChange={(e) => setManual(e.target.value)}
              placeholder="Or paste an image URL…"
              className="w-56"
              onKeyDown={(e) => {
                if (e.key === "Enter" && manualOk) choose(manualTrimmed);
              }}
            />
            <Button
              variant="secondary"
              disabled={!manualOk || saving !== null}
              loading={saving === manualTrimmed}
              onClick={() => choose(manualTrimmed)}
            >
              Use
            </Button>
          </div>
        </div>
      }
    >
      <div className="flex flex-col gap-3">
        <p className="text-caption text-muted-foreground">
          The image on this source's gallery card. These are the images on the
          page itself — pick one, or paste any image URL below.
        </p>
        {candidates === null ? (
          <div className="flex items-center gap-2 py-6 text-caption text-muted-foreground">
            <Spinner className="h-3.5 w-3.5" /> Reading the page…
          </div>
        ) : visible.length === 0 ? (
          <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
            {fetchFailed
              ? "Couldn't reach the page — paste an image URL below instead."
              : "No images found on the page — paste an image URL below instead."}
          </div>
        ) : (
          <div className="grid grid-cols-3 gap-2">
            {visible.map((u) => (
              <button
                key={u}
                type="button"
                onClick={() => choose(u)}
                disabled={saving !== null}
                title={u}
                className={cn(
                  "relative overflow-hidden rounded-md border bg-surface-2 transition-colors",
                  u === source.imageUrl
                    ? "border-primary"
                    : "border-border hover:border-border-strong",
                )}
              >
                <img
                  src={u}
                  alt=""
                  loading="lazy"
                  onError={() => setBroken((m) => ({ ...m, [u]: true }))}
                  className="h-24 w-full object-cover"
                />
                {saving === u && (
                  <span className="absolute inset-0 flex items-center justify-center bg-surface/60">
                    <Spinner className="h-4 w-4" />
                  </span>
                )}
              </button>
            ))}
          </div>
        )}
      </div>
    </Modal>
  );
}
