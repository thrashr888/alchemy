import { useEffect, useState } from "react";
import { Rss } from "lucide-react";
import { Button, LoadingState, Modal } from "./ui";
import { api } from "@/lib/api";
import { useStore } from "@/lib/store";
import type { FeedCandidate, Source } from "@/lib/types";

const TIER_HINT: Record<FeedCandidate["tier"], string> = {
  page: "advertised by the page",
  host: "from the site\u2019s shape",
  "well-known": "found at a conventional path",
};

/** "Follow updates\u2026" for a web source (docs/RFC-events.md \u00a72): every feed
 *  the app can offer, each one click from becoming a living feed source.
 *  Discovery may probe the origin's conventional paths, so it runs only
 *  while this modal is open \u2014 never from a sweep. */
export function FollowFeedModal({
  source,
  onClose,
}: {
  source: Source | null;
  onClose: () => void;
}) {
  const [found, setFound] = useState<FeedCandidate[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const sources = useStore((s) => s.sources);
  const addSourceUrl = useStore((s) => s.addSourceUrl);
  const followed = new Set(
    sources.filter((s) => s.sourceType === "feed").map((s) => s.url.replace(/\/$/, "")),
  );

  useEffect(() => {
    if (!source) return;
    setFound(null);
    let live = true;
    api
      .discoverFeeds(source.id)
      .then((list) => live && setFound(list))
      .catch(() => live && setFound([]));
    return () => {
      live = false;
    };
  }, [source]);

  const follow = async (c: FeedCandidate) => {
    setBusy(c.url);
    try {
      await addSourceUrl(c.url);
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal
      open={!!source}
      onClose={onClose}
      title="Follow updates"
      width="max-w-md"
    >
      <div className="flex flex-col gap-3">
        <p className="text-caption text-muted-foreground">
          Feeds for <span className="text-foreground">{source?.title}</span>. Following one adds a
          living source whose new entries arrive on their own.
        </p>
        {found === null ? (
          <LoadingState label="Looking for feeds\u2026" />
        ) : found.length === 0 ? (
          <div className="rounded-md border border-dashed border-border px-3 py-2.5 text-caption text-subtle-foreground">
            This page advertises no feed and its site has none at the usual paths.
          </div>
        ) : (
          <div className="flex flex-col gap-1.5">
            {found.map((c) => {
              const already = followed.has(c.url.replace(/\/$/, ""));
              return (
                <div
                  key={c.url}
                  className="flex items-center gap-2 rounded-md border border-border px-3 py-2"
                >
                  <Rss className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-body text-foreground">{c.label}</div>
                    <div className="truncate text-caption text-muted-foreground" title={c.url}>
                      {c.url.replace(/^https?:\/\//, "")} \u00b7 {TIER_HINT[c.tier]}
                    </div>
                  </div>
                  <Button
                    variant="ghost"
                    disabled={already || busy !== null}
                    onClick={() => void follow(c)}
                    title={already ? "Already followed in this notebook" : "Follow this feed"}
                  >
                    {already ? "Following" : busy === c.url ? "Following\u2026" : "Follow"}
                  </Button>
                </div>
              );
            })}
          </div>
        )}
        <div className="flex justify-end pt-1">
          <Button type="button" variant="ghost" onClick={onClose}>
            Done
          </Button>
        </div>
      </div>
    </Modal>
  );
}
