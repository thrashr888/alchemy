// Lifecycle and trust chips for OKF concepts (docs/RFC-okf-live.md §4).
// A bundle says three things about each of its documents that the app should
// not have to guess: whether it is still current, when it goes out of date,
// and who has checked it. Chips, not colored edges — identity and status ride
// in dots and chips here (DESIGN.md §2).
import { useStore } from "@/lib/store";
import type { OkfLifecycle } from "@/lib/types";

/** Is this concept past the date its bundle said it would go out of date? */
export function isStale(life: OkfLifecycle | undefined): boolean {
  return !!life && life.staleAfter > 0 && life.staleAfter < Date.now();
}

const TRUST_LABEL: Record<string, string> = {
  machine: "Checked",
  human: "Reviewed",
};

const TRUST_TITLE: Record<string, string> = {
  machine: "A tool confirmed this concept.",
  human: "A person reviewed this concept.",
};

function Chip({
  tone,
  label,
  title,
}: {
  tone: "muted" | "warning";
  label: string;
  title: string;
}) {
  return (
    <span
      title={title}
      className={
        tone === "warning"
          ? "shrink-0 rounded border border-warning/40 bg-warning/10 px-1.5 py-px text-micro text-warning"
          : "shrink-0 rounded border border-border px-1.5 py-px text-micro text-muted-foreground"
      }
    >
      {label}
    </span>
  );
}

/**
 * The chips one concept earns. Renders nothing when the bundle said nothing,
 * which is every source that is not part of one.
 */
export function OkfBadges({ sourceId }: { sourceId: string }) {
  const life = useStore((s) => s.okfLifecycle[sourceId]);
  if (!life) return null;
  const stale = isStale(life);
  const trust = TRUST_LABEL[life.trust];
  if (!stale && !trust && life.status !== "deprecated" && life.status !== "draft")
    return null;
  return (
    <>
      {life.status === "deprecated" && (
        <Chip
          tone="muted"
          label="Deprecated"
          title="The bundle retired this concept. It stays readable and stays out of answers until you tick it back on."
        />
      )}
      {life.status === "draft" && (
        <Chip
          tone="muted"
          label="Draft"
          title="The bundle marks this concept a draft."
        />
      )}
      {stale && (
        <Chip
          tone="warning"
          label="Stale"
          title={`The bundle said this concept goes out of date on ${new Date(
            life.staleAfter,
          ).toLocaleDateString()}.`}
        />
      )}
      {trust && <Chip tone="muted" label={trust} title={TRUST_TITLE[life.trust]} />}
    </>
  );
}
