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

/**
 * A deletion the other person in a shared notebook made, still unanswered
 * (docs/RFC-shared-notebook.md §3). Between two people a deletion is a
 * proposal: the source or note is still here, still readable, and the row
 * says who asked and offers both answers. Renders nothing — for every
 * notebook that is not shared, and every row nobody deleted.
 *
 * No confirmation on Remove (DESIGN.md §9): the deletion already happened on
 * their Mac, this only agrees with it, and Restore is the way back.
 */
export function DeletionProposalMark({ id }: { id: string }) {
  const proposal = useStore((s) => s.deletionProposals[id]);
  const resolve = useStore((s) => s.resolveDeletionProposal);
  if (!proposal) return null;
  return (
    <span className="pointer-events-auto relative z-20 flex flex-wrap items-center gap-1.5 text-micro text-muted-foreground">
      <Chip
        tone="muted"
        label={`Deleted by ${proposal.by}`}
        title={`${proposal.by} deleted this in the shared folder. It stays here until you answer.`}
      />
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          void resolve(id, true);
        }}
        className="rounded px-1 py-0.5 text-subtle-foreground hover:text-foreground"
        title="Put it back for both of you"
      >
        Restore
      </button>
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          void resolve(id, false);
        }}
        className="rounded px-1 py-0.5 text-subtle-foreground hover:text-foreground"
        title="Agree, and remove it here too"
      >
        Remove
      </button>
    </span>
  );
}
