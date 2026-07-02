import { useEffect, useState } from "react";
import { useStore } from "@/lib/store";
import { Modal, Button, Textarea } from "./ui";
import { cn } from "@/lib/utils";
import { type ChatConfig } from "@/lib/types";

const STYLES = [
  { id: "default", label: "Default", hint: "Balanced, grounded answers for research and brainstorming." },
  { id: "learning", label: "Learning Guide", hint: "Explains step by step, defines terms, builds intuition." },
  { id: "custom", label: "Custom", hint: "Give your own goal, style, or role." },
] as const;

const LENGTHS = [
  { id: "default", label: "Default" },
  { id: "longer", label: "Longer" },
  { id: "shorter", label: "Shorter" },
] as const;

export function ConfigureChatModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const chatConfig = useStore((s) => s.chatConfig);
  const setChatConfig = useStore((s) => s.setChatConfig);
  const [draft, setDraft] = useState<ChatConfig>(chatConfig);

  useEffect(() => {
    if (open) setDraft(chatConfig);
  }, [open, chatConfig]);

  const styleHint = STYLES.find((s) => s.id === draft.style)?.hint;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Configure chat"
      width="max-w-lg"
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              setChatConfig(draft);
              onClose();
            }}
          >
            Save
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-[13px] leading-relaxed text-muted-foreground">
          Tune how the assistant responds in this notebook — its conversational goal and how much
          detail it gives.
        </p>

        <div className="flex flex-col gap-1.5">
          <label className="text-[12px] font-medium text-foreground">
            Conversational goal, style, or role
          </label>
          <div className="flex flex-wrap gap-1.5">
            {STYLES.map((s) => (
              <Pill
                key={s.id}
                active={draft.style === s.id}
                onClick={() => setDraft({ ...draft, style: s.id })}
              >
                {s.label}
              </Pill>
            ))}
          </div>
          {styleHint && <span className="text-[11px] text-subtle-foreground">{styleHint}</span>}
          {draft.style === "custom" && (
            <Textarea
              rows={4}
              autoFocus
              className="mt-1"
              placeholder="e.g. Act as a skeptical peer reviewer; challenge claims and ask for evidence."
              value={draft.customPrompt}
              onChange={(e) => setDraft({ ...draft, customPrompt: e.target.value })}
            />
          )}
        </div>

        <div className="flex flex-col gap-1.5">
          <label className="text-[12px] font-medium text-foreground">Response length</label>
          <div className="flex flex-wrap gap-1.5">
            {LENGTHS.map((l) => (
              <Pill
                key={l.id}
                active={draft.length === l.id}
                onClick={() => setDraft({ ...draft, length: l.id })}
              >
                {l.label}
              </Pill>
            ))}
          </div>
        </div>
      </div>
    </Modal>
  );
}

function Pill({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "rounded-md border px-3 py-1.5 text-[12.5px] transition-colors",
        active
          ? "border-primary/60 bg-primary/15 text-citation"
          : "border-border bg-surface-2 text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
