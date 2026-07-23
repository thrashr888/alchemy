import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { Markdown } from "./Markdown";
import { cn } from "@/lib/utils";
import { Download } from "lucide-react";

/** The episode player for an Audio Overview note, with a save-a-copy button
 *  (null while loading or if the episode file is missing — e.g. the note
 *  predates this feature). */
export function AudioPlayer({ noteId, title }: { noteId: string; title: string }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    api
      .getAudioPath(noteId)
      .then((p) => {
        if (!stale) setSrc(p ? convertFileSrc(p) : null);
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [noteId]);
  if (!src) return null;

  async function download() {
    const { pushToast } = useStore.getState();
    const safe = title.replace(/[/\\:]/g, "-").trim() || "Audio Overview";
    const dest = await save({
      defaultPath: `${safe}.m4a`,
      filters: [{ name: "Audio", extensions: ["m4a"] }],
    });
    if (!dest) return;
    try {
      await api.exportAudio(noteId, dest);
      pushToast("success", "Episode saved");
    } catch (e) {
      pushToast("error", e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="flex items-center gap-1.5">
      <audio controls className="w-full min-w-0 flex-1" src={src} />
      <button
        className="shrink-0 rounded-md p-2 text-muted-foreground transition-colors hover:bg-surface-2 hover:text-foreground"
        onClick={() => void download()}
        title="Save the episode as an audio file"
        aria-label="Download episode audio"
      >
        <Download className="h-4 w-4" />
      </button>
    </div>
  );
}

const LINE = /^[\s*#>-]*?(host|guest)\b[\s*:—-]+(.+)$/i;

/** Render a HOST/GUEST script as a readable dialogue; falls back to plain
 *  markdown when the content doesn't parse as one. */
export function DialogueScript({ content }: { content: string }) {
  const lines = content
    .split("\n")
    .map((l) => LINE.exec(l.trim()))
    .filter((m): m is RegExpExecArray => !!m);
  if (lines.length === 0) return <Markdown>{content}</Markdown>;
  return (
    <div className="flex flex-col gap-3">
      {lines.map((m, i) => {
        const host = m[1].toLowerCase() === "host";
        return (
          <div key={i} className="flex gap-2.5">
            <span
              className={cn(
                "mt-0.5 h-fit shrink-0 rounded px-1.5 py-0.5 text-badge font-semibold uppercase tracking-wide",
                host ? "bg-primary/15 text-citation" : "bg-surface-2 text-muted-foreground",
              )}
            >
              {host ? "Host" : "Guest"}
            </span>
            <p className="text-body leading-relaxed text-foreground/90">
              {m[2].replace(/[*_`]/g, "")}
            </p>
          </div>
        );
      })}
    </div>
  );
}
