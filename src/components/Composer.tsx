import { useEffect } from "react";
import { cn } from "@/lib/utils";
import { Button, Textarea } from "./ui";
import { ArrowUp, Paperclip, Square } from "lucide-react";

/** Composer autosize ceiling — past this the textarea scrolls instead. */
export const COMPOSER_MAX_H = 180;

export interface ComposerProps {
  value: string;
  onChange: (value: string) => void;
  /** Enter-to-send, arrow navigation for a picker, Escape to dismiss one —
   *  what a keystroke does is specific to the surface asking, so the caller
   *  supplies the whole handler rather than Composer guessing at Enter. */
  onKeyDown: (e: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  onBlur?: () => void;
  /** The textarea's own ref — the caller needs it too (focus after a picker
   *  pick, cursor placement, warming a cold model on first keystroke), so
   *  Composer takes the object rather than making its own. */
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  placeholder: string;
  /** Nothing to answer into yet (no notebook, no sources) — dims the field
   *  and the send button both. */
  disabled?: boolean;
  /** An answer is being written into this conversation right now — swaps
   *  Send for Stop. */
  sending: boolean;
  onSubmit: () => void;
  onStop?: () => void;
  /** The composer's one pop-up for how the answer gets made — a `ModelPill`
   *  or a `MenuPill` built by the caller, so Composer stays decoupled from
   *  what it configures (DESIGN.md §4, "the chat composer is its own
   *  component"). */
  menu: React.ReactNode;
  /** Attach a source. Omitted entirely — not just disabled — on a surface
   *  with nothing to attach to (Home's corpus conversation). */
  onAttach?: () => void;
  attachDisabled?: boolean;
  attachTitle?: string;
  /** Slash/mention picker overlays. Both float `absolute bottom-full` off
   *  this box, so where they sit in the tree doesn't matter — only that
   *  they're inside it. */
  pickers?: React.ReactNode;
  /** A quiet line under the input, above the button row (the notebook
   *  composer's "Searching only: …" mention list). */
  note?: React.ReactNode;
  role?: string;
  ariaExpanded?: boolean;
  ariaControls?: string;
  ariaActiveDescendant?: string;
  className?: string;
}

/**
 * The chat composer (docs/RFC-mac-chrome.md, "Sheet (chat)"): 680 wide,
 * radius 22, frosted like a menu, a strong hairline and a deep shadow to
 * lift it off the transcript, `pt-2.5 pr-2.5 pb-2 pl-4`. One shape serves
 * both the notebook's Chat page (`ChatPanel`) and Home's corpus
 * conversation (`HomeView`'s Chats section) — same growing textarea, same
 * button row, same one pop-up for how the answer gets made — because a
 * second hand-drawn copy is exactly the kind of divergence DESIGN.md §4's
 * consistency ledger exists to catch.
 *
 * Composer owns layout and the growing-textarea mechanics only. Everything
 * that makes one surface's chat different from the other's — slash
 * commands, @-mentions, streaming, what the pop-up itself offers — is the
 * caller's wiring, handed in as props or built into the `menu`/`pickers`
 * slots.
 */
export function Composer({
  value,
  onChange,
  onKeyDown,
  onBlur,
  textareaRef,
  placeholder,
  disabled,
  sending,
  onSubmit,
  onStop,
  menu,
  onAttach,
  attachDisabled,
  attachTitle = "Attach a source",
  pickers,
  note,
  role,
  ariaExpanded,
  ariaControls,
  ariaActiveDescendant,
  className,
}: ComposerProps) {
  // Autosize from the value, not from onChange: sending, a slash reset, a
  // follow-up click and retry-after-failure all set the text programmatically
  // in the caller, so keying off the value is what makes the box shrink back
  // as well as grow.
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_H)}px`;
  }, [value, textareaRef]);

  return (
    <div
      className={cn(
        "menu-glass relative rounded-[22px] border border-border-strong transition-colors",
        "pt-2.5 pr-2.5 pb-2 pl-4 shadow-[0_12px_32px_rgba(0,0,0,.45)]",
        "focus-within:border-ring/60",
        className,
      )}
    >
      {pickers}
      <Textarea
        ref={textareaRef}
        rows={1}
        // 14px: the one place in the app where the user's own words are
        // bigger than the chrome around them. The composer's own padding
        // places the text, so the field adds none.
        className="border-0 bg-transparent p-0 text-[14px] focus:ring-0 min-h-[24px] max-h-[180px]"
        placeholder={placeholder}
        value={value}
        disabled={disabled}
        role={role}
        aria-expanded={ariaExpanded}
        aria-controls={ariaControls}
        aria-activedescendant={ariaActiveDescendant}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
        onKeyDown={onKeyDown}
      />
      {note}
      <div className="mt-2 flex items-center gap-1.5">
        {menu}
        {/* Attach: the notebook's own Add Source, where a chat app would put
            a paperclip — what you attach here is a source. Home's corpus
            chat has nothing to attach to, so it passes no handler and the
            button never renders. */}
        {onAttach && (
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6 rounded-full"
            disabled={attachDisabled}
            onClick={onAttach}
            title={attachTitle}
            aria-label={attachTitle}
          >
            <Paperclip className="h-3.5 w-3.5" />
          </Button>
        )}
        <span className="flex-1" />
        {sending ? (
          <Button
            variant="secondary"
            size="icon"
            className="rounded-full"
            onClick={onStop}
            title="Stop"
            aria-label="Stop generating"
          >
            <Square className="h-3.5 w-3.5" />
          </Button>
        ) : (
          <Button
            variant="primary"
            size="icon"
            className="rounded-full"
            onClick={onSubmit}
            disabled={!value.trim() || disabled}
            title="Send"
            aria-label="Send message"
          >
            <ArrowUp className="h-4 w-4" />
          </Button>
        )}
      </div>
    </div>
  );
}
