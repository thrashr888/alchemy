import { Button } from "./ui";
import type { AcpPermissionEvent } from "@/lib/types";

/** A hosted agent asking to run a tool (docs/RFC-acp-agents.md). Shared by
 *  the notebook's Agent pane and Home chat, where the agent can also answer
 *  a turn: the question is the same wherever the agent is working, so the
 *  answer to it looks the same too. The options are the agent's own — allow
 *  once, always, reject — and Cancel declines without picking one. */
export function PermissionPrompt({
  request,
  agent,
  onAnswer,
}: {
  request: AcpPermissionEvent;
  /** Who is asking ("Claude Code"), when the caller knows. */
  agent?: string | null;
  onAnswer: (optionId: string | null) => void;
}) {
  // The loop is Alchemy's own model; an agent turn names its agent.
  const who = request.fromLoop ? "Alchemy" : agent || "The agent";
  return (
    <div className="rounded-lg border border-border bg-surface-2 px-3 py-2.5">
      <p className="text-caption text-foreground">
        {request.action ? (
          // One of Alchemy's own tools: say what it does, not what it's
          // called on the wire.
          <>
            {who} wants to {request.action}.
          </>
        ) : (
          // Anything else is the agent's own tool; its name is the best
          // description there is.
          <>
            {who} wants to run{" "}
            <span className="font-mono">{request.toolTitle || "a tool"}</span>.
          </>
        )}
      </p>
      {request.detail && request.detail.length > 0 && (
        // What exactly would change, one URL per line: selectable, with the
        // full address on hover when a long one is cut short.
        <ul className="mt-1.5 flex flex-col gap-0.5 text-caption text-muted-foreground">
          {request.detail.map((line) => (
            <li key={line} className="selectable truncate font-mono" title={line}>
              {line}
            </li>
          ))}
        </ul>
      )}
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
