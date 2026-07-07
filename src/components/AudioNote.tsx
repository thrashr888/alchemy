import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "@/lib/api";
import { Markdown } from "./Markdown";
import { cn } from "@/lib/utils";

/** The episode player for an Audio Overview note (null while loading or if
 *  the episode file is missing — e.g. the note predates this feature). */
export function AudioPlayer({ noteId }: { noteId: string }) {
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
  return <audio controls className="w-full" src={src} />;
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
                "mt-0.5 h-fit shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide",
                host ? "bg-primary/15 text-citation" : "bg-surface-2 text-muted-foreground",
              )}
            >
              {host ? "Host" : "Guest"}
            </span>
            <p className="text-[13px] leading-relaxed text-foreground/90">
              {m[2].replace(/[*_`]/g, "")}
            </p>
          </div>
        );
      })}
    </div>
  );
}
