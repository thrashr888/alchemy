import { Button } from "./ui";
import type { AcpPermissionEvent } from "@/lib/types";

/** A hosted agent asking to run a tool (docs/RFC-acp-agents.md). Shared by
 *  the notebook's Agent pane and Home chat, where the agent can also answer
 *  a turn: the question is the same wherever the agent is working, so the
 *  answer to it looks the same too. The options are the agent's own — allow
 *  once, always, reject — and Cancel declines without picking one. */
export function PermissionPrompt({
  request,
  onAnswer,
}: {
  request: AcpPermissionEvent;
  onAnswer: (optionId: string | null) => void;
}) {
  return (
    <div className="rounded-lg border border-border bg-surface-2 px-3 py-2.5">
      <p className="text-caption text-foreground">
        The agent wants to run{" "}
        <span className="font-mono">{request.toolTitle || "a tool"}</span>.
      </p>
      <div className="mt-2 flex flex-wrap gap-1.5">
        {request.options.map((opt) => (
          <Button
            key={opt.id}
            size="sm"
            variant={opt.kind.startsWith("allow") ? "primary" : "secondary"}
            onClick={() => onAnswer(opt.id)}
          >
            {opt.name}
          </Button>
        ))}
        <Button size="sm" variant="ghost" onClick={() => onAnswer(null)}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
