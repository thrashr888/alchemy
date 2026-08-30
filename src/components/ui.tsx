import * as React from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/utils";
import {
  Check,
  Loader2,
  MoreHorizontal,
  X,
  CheckCircle2,
  AlertTriangle,
  Info,
} from "lucide-react";
import type { Toast } from "@/lib/types";

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
type ButtonSize = "sm" | "md" | "icon";

const variants: Record<ButtonVariant, string> = {
  primary:
    "bg-primary text-primary-foreground hover:bg-primary-hover shadow-[0_1px_2px_rgba(0,0,0,0.3)]",
  secondary:
    "bg-surface-2 text-foreground hover:bg-elevated border border-border-strong",
  ghost: "text-muted-foreground hover:text-foreground hover:bg-surface-2",
  danger: "bg-destructive/10 text-destructive hover:bg-destructive/20",
};

const sizes: Record<ButtonSize, string> = {
  sm: "h-7 px-2.5 text-caption gap-1.5 rounded-md",
  md: "h-8 px-3 text-body gap-2 rounded-md",
  icon: "h-7 w-7 rounded-md justify-center",
};

export function Button({
  variant = "secondary",
  size = "md",
  className,
  loading,
  children,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  ref?: React.Ref<HTMLButtonElement>;
}) {
  return (
    <button
      // Untyped buttons are SUBMIT buttons: inside a form, Enter would
      // "click" the first one. Callers that mean submit pass type="submit".
      type="button"
      className={cn(
        "inline-flex items-center whitespace-nowrap font-medium transition-colors select-none outline-none",
        "focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-50 disabled:pointer-events-none",
        variants[variant],
        sizes[size],
        className,
      )}
      disabled={loading || props.disabled}
      {...props}
    >
      {loading && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
      {children}
    </button>
  );
}

export function Input({
  className,
  ...props
}: React.InputHTMLAttributes<HTMLInputElement> & {
  ref?: React.Ref<HTMLInputElement>;
}) {
  return (
    <input
      className={cn(
        "h-8 w-full rounded-md bg-surface-2 px-2.5 text-body text-foreground",
        "border border-input placeholder:text-subtle-foreground outline-none",
        "focus:border-ring/70 focus:ring-1 focus:ring-ring/40 transition-colors",
        className,
      )}
      // Inputs here hold titles, URLs, tags, and filters — not prose. macOS
      // autocorrect mangles identifiers and Writing Tools' popover button is
      // noise in chrome; both default off (callers can re-enable per field).
      autoComplete="off"
      autoCorrect="off"
      spellCheck={false}
      {...({ writingsuggestions: "false" } as Record<string, string>)}
      {...props}
    />
  );
}

export function Textarea({
  className,
  ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement> & {
  ref?: React.Ref<HTMLTextAreaElement>;
}) {
  return (
    <textarea
      className={cn(
        "w-full rounded-md bg-surface-2 px-2.5 py-2 text-body text-foreground resize-none",
        "border border-input placeholder:text-subtle-foreground outline-none",
        "focus:border-ring/70 focus:ring-1 focus:ring-ring/40 transition-colors",
        className,
      )}
      // Prose fields keep spellcheck, but the macOS Writing Tools popover
      // button (the "Siri" affordance in every focused field) stays out.
      autoComplete="off"
      {...({ writingsuggestions: "false" } as Record<string, string>)}
      {...props}
    />
  );
}

export function Spinner({ className }: { className?: string }) {
  // Decoration: the words beside it say what is happening.
  return <Loader2 aria-hidden className={cn("animate-spin", className)} />;
}

export interface HoverCardData {
  title: string;
  /** Right-aligned beside the title (e.g. a relative time). */
  time?: string;
  meta: { icon?: React.ReactNode; label: string; value?: string }[];
}

/** The quiet-row hover pattern: rows show name and status at rest; hovering
 *  a beat floats an info card beside the list (sidebar-adjacent, like
 *  ChatGPT desktop's nav). One hook per LIST — rows pass their data to
 *  `show` in onMouseEnter — so it works inside .map without per-row hooks.
 *  The card is info-only and pointer-events-none: actions stay on the row. */
export function useHoverCard(side: "left" | "right") {
  const [state, setState] = React.useState<{
    top: number;
    left: number;
    data: HoverCardData;
  } | null>(null);
  const timer = React.useRef<number | undefined>(undefined);
  // Warm-tooltip behavior (native macOS): only the FIRST reveal waits a
  // beat; while a card is up, moving to the next row switches instantly.
  // `hide` grace keeps it warm across the row gap.
  const warm = React.useRef(false);

  // `Element`, not `HTMLElement`: the graph's rows are SVG <g> nodes, and
  // all this needs is getBoundingClientRect.
  const show = (e: React.MouseEvent<Element>, data: HoverCardData) => {
    const el = e.currentTarget;
    window.clearTimeout(timer.current);
    const reveal = () => {
      const r = el.getBoundingClientRect();
      warm.current = true;
      setState({
        top: r.top,
        left: side === "right" ? r.right + 10 : r.left - 10,
        data,
      });
    };
    if (warm.current) reveal();
    else timer.current = window.setTimeout(reveal, 450);
  };
  const hide = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      warm.current = false;
      setState(null);
    }, 120);
  };
  // Scrolling the list under the cursor won't fire mouseleave — drop the
  // card on any scroll instead of letting it drift from its row.
  React.useEffect(() => {
    if (!state) return;
    const drop = () => {
      warm.current = false;
      setState(null);
    };
    window.addEventListener("scroll", drop, true);
    return () => window.removeEventListener("scroll", drop, true);
  }, [state]);

  const card = state
    ? createPortal(
        <div
          // The reader's live web view is a NATIVE child webview: it paints
          // above every HTML layer, whatever the z-index. `data-overlay`
          // is the marker it watches for to hide itself (see ReaderPane).
          data-overlay=""
          className={cn(
            "pointer-events-none fixed z-[100] w-64 rounded-lg border border-border-strong bg-surface-2 p-3 shadow-2xl",
            side === "left" && "-translate-x-full",
          )}
          style={{
            top: Math.max(8, Math.min(state.top, window.innerHeight - 200)),
            left: state.left,
          }}
        >
          <div className="flex items-baseline gap-2">
            <span className="min-w-0 truncate text-body font-medium text-foreground">
              {state.data.title}
            </span>
            {state.data.time && (
              <span className="ml-auto shrink-0 text-micro text-subtle-foreground">
                {state.data.time}
              </span>
            )}
          </div>
          {state.data.meta.length > 0 && (
            <div className="mt-2 flex flex-col gap-1.5">
              {state.data.meta.map((m: HoverCardData["meta"][number], i: number) => (
                <div
                  key={i}
                  className="flex min-w-0 items-start gap-2 text-caption"
                >
                  {m.icon && (
                    <span className="shrink-0 text-muted-foreground [&_svg]:h-3.5 [&_svg]:w-3.5">
                      {m.icon}
                    </span>
                  )}
                  <span className="min-w-0 truncate text-muted-foreground">
                    {m.label}
                  </span>
                  {m.value && (
                    // Values wrap (file paths, URLs) rather than clipping
                    // out of the fixed-width card.
                    <span className="ml-auto min-w-0 break-all pl-2 text-right text-foreground">
                      {m.value}
                    </span>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>,
        document.body,
      )
    : null;

  return { show, hide, card };
}

/** macOS-style toggle switch for settings rows: a pill track with a sliding
 *  thumb instead of a web checkbox. The real input stays underneath for
 *  keyboard focus and screen readers. */
export function Switch({
  checked,
  onChange,
  className,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  className?: string;
}) {
  return (
    // The pill is 36×16 — an uncomfortably small target on its own, so the
    // wrapper pads a hit halo around it (negative margin keeps layout
    // unchanged) and the invisible input covers the padded box. Padding
    // beats a negative-inset input: the box the browser hit-tests is the
    // wrapper's own, reliable in every stacking context.
    <span className={cn("relative inline-flex shrink-0 p-2 -m-2", className)}>
      <input
        type="checkbox"
        role="switch"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        // h/w-full is load-bearing: a checkbox keeps its intrinsic ~12px
        // box under `inset-0` alone (form controls don't stretch), which
        // left the real hit area a corner of the pill.
        className="peer absolute inset-0 h-full w-full cursor-pointer rounded-full opacity-0"
      />
      {/* Native NSSwitch geometry, measured off a rendered control (dark
          aqua): 2.25:1 pill, knob a 1.6:1 capsule spanning 59% of the track
          width with an ~8% inset. At our scale: 36×16 track, 21×13 knob,
          1.5px inset, 12px travel. In dark themes the knob is translucent,
          picking up the track's tint like the native glass knob; in light
          themes it stays solid white. */}
      <span
        aria-hidden
        className={cn(
          "pointer-events-none flex h-4 w-9 shrink-0 items-center rounded-full p-[1.5px]",
          "transition-colors duration-200 ease-out",
          "peer-focus-visible:ring-2 peer-focus-visible:ring-ring/60",
          checked ? "bg-primary" : "bg-muted-foreground/35",
        )}
      >
        <span
          className={cn(
            "h-[13px] w-[21px] rounded-full bg-white [[data-scheme=dark]_&]:bg-white/85",
            "shadow-[0_0_0_0.5px_rgba(0,0,0,0.05),0_1px_1px_rgba(0,0,0,0.16)]",
            "transition-transform duration-200 ease-out",
            checked && "translate-x-3",
          )}
        />
      </span>
    </span>
  );
}

/**
 * Full-card primary action for cards that also contain sibling controls.
 * The button is a sibling, not a wrapper, so menus and checkboxes never become
 * nested interactive content. Place it inside a `relative` card and keep
 * secondary controls above it with `relative z-20`. (RowMenu dropdowns
 * render in a body portal, so no stacking-context bumps are needed.)
 */
export function CardAction({
  label,
  onClick,
  className,
}: {
  label: string;
  /** Receives the click event so hosts can branch on modifier keys
   *  (shift/cmd selection, RFC-multi-select). */
  onClick: (e: React.MouseEvent<HTMLButtonElement>) => void;
  className?: string;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      // Marks this as the row's own surface, not a control: the marquee
      // hook lets rubber-band drags start here (Finder-style) while real
      // controls (menus, checkboxes) still block them.
      data-card-action
      className={cn(
        "absolute inset-0 z-0 rounded-[inherit] outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
        className,
      )}
    />
  );
}

/**
 * Finder-style rubber-band selection over a scrollable list
 * (RFC-multi-select). Attach the returned handlers to the scroll container
 * and stamp each selectable row with `data-pick-id`. The drag draws a fixed
 * overlay rectangle and reports the intersecting ids on every move
 * (additive when shift/cmd is held). A sub-threshold press on empty space
 * clears the selection; a drag that actually started suppresses the click
 * that follows it — check `justEnded()` in row click handlers.
 */
/** The element that actually scrolls for a given container — itself, or
 *  the nearest ancestor that overflows. The notes list is a plain div inside
 *  Studio's scrolling column, so auto-scroll has to walk up to find the
 *  thing with a scrollbar rather than assume the container has one. */
function scrollParent(el: HTMLElement): HTMLElement {
  for (let node: HTMLElement | null = el; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (
      /(auto|scroll|overlay)/.test(style.overflowY) &&
      node.scrollHeight > node.clientHeight
    )
      return node;
  }
  return el;
}

export function useMarquee({
  containerRef,
  onStart,
  onSelect,
  onClearBackground,
}: {
  containerRef: React.RefObject<HTMLElement | null>;
  /** Fires once when a drag passes the threshold — hosts snapshot the
   *  pre-drag selection here so an additive drag unions against the
   *  selection as it was, not as the drag mutates it. */
  onStart?: (additive: boolean) => void;
  onSelect: (ids: string[], additive: boolean) => void;
  onClearBackground?: () => void;
}) {
  const [rect, setRect] = React.useState<{
    x: number;
    y: number;
    w: number;
    h: number;
  } | null>(null);
  const endedAt = React.useRef(0);

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    const t = e.target as HTMLElement;
    // Real controls keep their gestures; the row surface (CardAction) and
    // true background both start a marquee.
    // A row that can be dragged out of the app (dragOut.ts) owns its own
    // drag: pressing it means "take this file", not "start a band". Finder
    // draws the same line — bands begin on background, never on an item.
    if (
      t.closest(
        "button:not([data-card-action]), input, a, textarea, select, [role='menu'], [data-drag-out]",
      )
    )
      return;
    const container = containerRef.current;
    if (!container) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    // The band is anchored to the CONTENT, not the viewport: when
    // auto-scroll moves the list under the cursor, the rectangle has to keep
    // growing from the row it started on, the way Finder does.
    const scroller = scrollParent(container);
    const scroll0 = scroller.scrollTop;
    const additive = e.shiftKey || e.metaKey || e.ctrlKey;
    const onBackground = !t.closest("[data-pick-id]");
    let started = false;
    let px = x0;
    let py = y0;
    let ticker: ReturnType<typeof setInterval> | undefined;

    const paint = () => {
      const anchorY = y0 - (scroller.scrollTop - scroll0);
      const x = Math.min(x0, px);
      const y = Math.min(anchorY, py);
      const w = Math.abs(px - x0);
      const h = Math.abs(py - anchorY);
      setRect({ x, y, w, h });
      const ids: string[] = [];
      container.querySelectorAll<HTMLElement>("[data-pick-id]").forEach((el) => {
        const r = el.getBoundingClientRect();
        if (r.left < x + w && r.right > x && r.top < y + h && r.bottom > y) {
          const id = el.getAttribute("data-pick-id");
          if (id) ids.push(id);
        }
      });
      onSelect(ids, additive);
    };

    // Drag past either edge and the list scrolls itself, faster the further
    // past you go — without it a selection can only ever be as tall as the
    // panel, which is the case a long source list most needs.
    const EDGE = 36;
    const MAX_SPEED = 18;
    const autoScroll = () => {
      if (!started) return;
      const box = scroller.getBoundingClientRect();
      let dy = 0;
      if (py < box.top + EDGE) {
        dy = -Math.ceil(((box.top + EDGE - py) / EDGE) * MAX_SPEED);
      } else if (py > box.bottom - EDGE) {
        dy = Math.ceil(((py - (box.bottom - EDGE)) / EDGE) * MAX_SPEED);
      }
      if (dy !== 0) {
        const before = scroller.scrollTop;
        scroller.scrollTop += dy;
        if (scroller.scrollTop !== before) paint();
      }
    };

    const move = (ev: PointerEvent) => {
      px = ev.clientX;
      py = ev.clientY;
      if (!started && Math.hypot(px - x0, py - y0) < 4) return;
      if (!started) {
        started = true;
        // Belt and braces against a native text selection: the rows are
        // already `select-none`, but a drag that begins over selectable
        // chrome would otherwise paint a text highlight under the band.
        document.body.style.userSelect = "none";
        window.getSelection()?.removeAllRanges();
        onStart?.(additive);
        // A timer rather than requestAnimationFrame: WKWebView suspends rAF
        // whenever the window isn't frontmost, and a drag that stops
        // scrolling because the app lost focus mid-gesture would be a
        // mystery to debug.
        ticker = setInterval(autoScroll, 16);
      }
      paint();
    };

    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (ticker) clearInterval(ticker);
      document.body.style.userSelect = "";
      setRect(null);
      if (started) endedAt.current = Date.now();
      else if (onBackground && !additive) onClearBackground?.();
    };

    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  /** True right after a drag finished — the click that follows it is the
   *  drag's tail, not an activation. */
  const justEnded = () => Date.now() - endedAt.current < 200;

  const marquee = rect
    ? createPortal(
        <div
          className="pointer-events-none fixed z-50 rounded-sm border border-primary/50 bg-primary/10"
          style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
        />,
        document.body,
      )
    : null;

  return { onPointerDown, marquee, justEnded };
}

/**
 * Drag strip on a side panel's inner edge for resizing. The panel must be
 * `position: relative`. Reports the desired panel width on every pointer
 * move; arrow keys nudge, double-click resets to the default width.
 */
export function ResizeHandle({
  edge,
  width,
  defaultWidth,
  onResize,
  label,
}: {
  /** Which edge of the panel the handle sits on. */
  edge: "right" | "left";
  width: number;
  defaultWidth: number;
  onResize: (width: number) => void;
  label: string;
}) {
  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const panel = e.currentTarget.parentElement;
    if (!panel) return;
    const rect = panel.getBoundingClientRect();
    const move = (ev: PointerEvent) => {
      onResize(
        edge === "right" ? ev.clientX - rect.left : rect.right - ev.clientX,
      );
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.style.cursor = "";
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    document.body.style.cursor = "col-resize";
  };
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onDoubleClick={() => onResize(defaultWidth)}
      onKeyDown={(e) => {
        const grow = edge === "right" ? "ArrowRight" : "ArrowLeft";
        const shrink = edge === "right" ? "ArrowLeft" : "ArrowRight";
        if (e.key === grow) onResize(width + 16);
        else if (e.key === shrink) onResize(width - 16);
        else return;
        e.preventDefault();
      }}
      className={cn(
        "group/resize absolute inset-y-0 z-20 w-1.5 cursor-col-resize transition-colors hover:bg-ring/30 active:bg-ring/40 focus-visible:bg-ring/30",
        // Fully inside the card edge: the panels clip at their rounded
        // border (overflow-hidden), so a straddling handle loses its
        // outer half to hit-testing.
        edge === "right" ? "right-0" : "left-0",
      )}
    >
      <span
        aria-hidden
        className="absolute top-1/2 left-1/2 flex -translate-x-1/2 -translate-y-1/2 flex-col gap-0.5 opacity-40 transition-opacity group-hover/resize:opacity-100 group-focus-visible/resize:opacity-100"
      >
        <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
        <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
        <span className="h-0.5 w-0.5 rounded-full bg-muted-foreground" />
      </span>
    </div>
  );
}

let modalSeq = 0;

/** Open modals, bottom to top. Escape must close only the topmost — every
 *  Modal listens on `window`, so without this a confirm stacked over
 *  Settings would take Settings down with it. */
const modalStack: symbol[] = [];

export function Modal({
  open,
  onClose,
  title,
  children,
  footer,
  headerActions,
  width = "max-w-md",
  tall = false,
  bodyScroll = true,
  hideHeader = false,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
  /** Icon buttons rendered in the title bar, left of the close X. */
  headerActions?: React.ReactNode;
  width?: string;
  /** Fill most of the window height (settings-style panes) instead of the
   *  compact dialog default. */
  tall?: boolean;
  /** Set false when the content manages its own scroll region (settings'
   *  fixed-sidebar layout) — nested scrollbars otherwise. */
  bodyScroll?: boolean;
  /** Skip the title bar (the caller renders the title inside its own
   *  layout, e.g. settings' nav header); the close X floats top-right and
   *  the dialog is labeled via aria-label instead. */
  hideHeader?: boolean;
}) {
  const panelRef = React.useRef<HTMLDivElement>(null);
  const titleId = React.useMemo(() => `modal-title-${++modalSeq}`, []);

  // Callers pass inline closures; keep the latest in a ref so the focus effect
  // below runs only on open/close, not on every parent re-render (which would
  // steal focus mid-typing).
  const onCloseRef = React.useRef(onClose);
  onCloseRef.current = onClose;

  React.useEffect(() => {
    if (!open) return;
    const stackToken = Symbol("modal");
    modalStack.push(stackToken);
    const trigger = document.activeElement as HTMLElement | null;
    // Focus the first form field if there is one (the header close button is
    // first in DOM order), else the first focusable, else the panel itself.
    const panel = panelRef.current;
    const focusable =
      panel?.querySelector<HTMLElement>("input,textarea,select") ??
      panel?.querySelector<HTMLElement>(
        'button,[tabindex]:not([tabindex="-1"])',
      );
    (focusable ?? panel)?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (modalStack[modalStack.length - 1] !== stackToken) return;
        onCloseRef.current();
        return;
      }
      // Trap Tab within the dialog.
      if (e.key === "Tab" && panel) {
        const items = Array.from(
          panel.querySelectorAll<HTMLElement>(
            'input,textarea,select,button,a[href],[tabindex]:not([tabindex="-1"])',
          ),
        ).filter((el) => !el.hasAttribute("disabled"));
        if (items.length === 0) return;
        const first = items[0];
        const last = items[items.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      const i = modalStack.indexOf(stackToken);
      if (i >= 0) modalStack.splice(i, 1);
      window.removeEventListener("keydown", onKey);
      // Restore focus to whatever opened the dialog — one tick later, so
      // the keystroke that closed it (Enter submitting a form) can't land
      // on the refocused trigger and re-activate it.
      const t = window.setTimeout(() => trigger?.focus?.(), 0);
      void t;
    };
  }, [open]);

  if (!open) return null;
  // Portaled to <body> like RowMenu: rendered inline, `fixed` gets re-scoped
  // by any transformed/filtered ancestor and the backdrop loses the z-battle
  // against sibling stacking contexts (the Home hero painted over confirm
  // dialogs opened from the content column).
  return createPortal(
    <div
      className={cn(
        "fixed inset-0 z-50 flex items-start justify-center bg-black/40 backdrop-blur-[2px] animate-in fade-in duration-150",
        tall ? "pt-[4vh]" : "pt-[12vh]",
      )}
      onMouseDown={onClose}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={hideHeader ? undefined : titleId}
        aria-label={hideHeader ? title : undefined}
        tabIndex={-1}
        className={cn(
          tall ? "max-h-[92vh]" : "max-h-[80vh]",
          "relative flex w-full flex-col rounded-lg bg-elevated outline-none animate-in zoom-in-95 duration-150",
          "shadow-[0_0_0_0.5px_var(--border-strong),0_16px_48px_-8px_rgba(0,0,0,0.45)]",
          width,
        )}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {hideHeader ? (
          <div className="absolute right-2 top-2 z-10 flex items-center gap-1">
            {headerActions}
            <Button
              variant="ghost"
              size="icon"
              onClick={onClose}
              aria-label="Close dialog"
            >
              <X className="h-4 w-4" />
            </Button>
          </div>
        ) : (
          <div className="flex min-h-11 shrink-0 items-center justify-between gap-3 border-b border-border px-4 py-2">
            <h2
              id={titleId}
              className="text-body font-semibold text-foreground"
            >
              {title}
            </h2>
            <div className="flex shrink-0 items-center gap-1">
              {headerActions}
              <Button
                variant="ghost"
                size="icon"
                onClick={onClose}
                aria-label="Close dialog"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
          </div>
        )}
        <div
          className={cn(
            "min-h-0 flex-1 p-4",
            // bodyScroll=false callers manage their own scroll column; the
            // body becomes a flex container so children size against real
            // flex constraints — percentage heights (h-full/max-h-full)
            // inside flex items silently fail in WKWebView.
            bodyScroll ? "overflow-y-auto" : "flex flex-col overflow-hidden",
          )}
        >
          {children}
        </div>
        {footer && (
          <div className="shrink-0 border-t border-border px-4 py-3">
            {footer}
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
}

/**
 * Promise-based confirmation using the app's Modal (not the native, un-themed
 * window.confirm). Returns `confirm(opts) => Promise<boolean>` plus a `dialog`
 * node to render once in the component.
 */
export function useConfirm() {
  const [state, setState] = React.useState<{
    title: string;
    message: string;
    items: string[];
    confirmLabel: string;
    danger: boolean;
    resolve: (ok: boolean) => void;
  } | null>(null);

  const confirm = React.useCallback(
    (opts: {
      title: string;
      message?: string;
      /** The things this will actually affect, listed by name. A count in
       *  the title says how many; only the list says which — and for a
       *  destructive action that is the difference between confirming and
       *  guessing. */
      items?: string[];
      confirmLabel?: string;
      danger?: boolean;
    }) =>
      new Promise<boolean>((resolve) => {
        setState({
          title: opts.title,
          message: opts.message ?? "",
          items: opts.items ?? [],
          confirmLabel: opts.confirmLabel ?? "Confirm",
          danger: opts.danger ?? false,
          resolve,
        });
      }),
    [],
  );

  const settle = (ok: boolean) => {
    state?.resolve(ok);
    setState(null);
  };

  const dialog = state ? (
    <Modal
      open
      onClose={() => settle(false)}
      title={state.title}
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={() => settle(false)}>
            Cancel
          </Button>
          <Button
            variant={state.danger ? "danger" : "primary"}
            onClick={() => settle(true)}
            autoFocus
          >
            {state.confirmLabel}
          </Button>
        </div>
      }
    >
      {state.message && (
        <p className="text-body leading-relaxed text-muted-foreground">
          {state.message}
        </p>
      )}
      {state.items.length > 0 && (
        <ul className="mt-2.5 max-h-52 overflow-y-auto rounded-md border border-border">
          {state.items.map((item, i) => (
            <li
              key={`${item}-${i}`}
              className="truncate border-b border-border px-2.5 py-1.5 text-caption text-foreground/90 last:border-b-0"
              title={item}
            >
              {item}
            </li>
          ))}
        </ul>
      )}
    </Modal>
  ) : null;

  return { confirm, dialog };
}

export interface Announcement {
  id: string | number;
  text: string;
}

/**
 * Screen-reader-only polite live region: the channel for state changes a
 * sighted user reads off the screen and a VoiceOver user would otherwise
 * miss. Two rules make it work:
 *
 * - It stays mounted for the life of its host. A region that appears
 *   together with its first message is announced unreliably — the region
 *   has to exist before the text lands in it.
 * - Each announcement is its own child node (`aria-atomic="false"`), so a
 *   sentence repeats aloud even when the words are identical to the last.
 *
 * Feed it transitions, never a stream: one entry per meaningful change.
 */
export function LiveRegion({
  announcements,
}: {
  announcements: Announcement[];
}) {
  return (
    <div
      role="status"
      aria-live="polite"
      aria-atomic="false"
      className="sr-only"
    >
      {announcements.map((a) => (
        <div key={a.id}>{a.text}</div>
      ))}
    </div>
  );
}

/** Bottom-center stack of ephemeral toasts. */
export function Toaster({
  toasts,
  onDismiss,
}: {
  toasts: Toast[];
  onDismiss: (id: string) => void;
}) {
  // The announcer renders unconditionally — the visible stack comes and goes,
  // but the live region it speaks through must already be in the document.
  // The stack itself is not the live region: its dismiss buttons would be
  // read out with every toast.
  const announcer = (
    <LiveRegion
      announcements={toasts.map((t) => ({ id: t.id, text: t.message }))}
    />
  );
  if (toasts.length === 0) return announcer;
  const icon = {
    success: (
      <CheckCircle2
        aria-hidden
        className="mt-0.5 h-4 w-4 shrink-0 text-success"
      />
    ),
    error: (
      <AlertTriangle
        aria-hidden
        className="mt-0.5 h-4 w-4 shrink-0 text-destructive"
      />
    ),
    info: <Info aria-hidden className="mt-0.5 h-4 w-4 shrink-0 text-citation" />,
  };
  const border = {
    success: "border-success/40",
    error: "border-destructive/40",
    info: "border-border-strong",
  };
  return (
    <>
      {announcer}
      <div className="pointer-events-none fixed bottom-[calc(1rem+env(safe-area-inset-bottom))] left-1/2 z-[70] flex -translate-x-1/2 flex-col items-center gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={cn(
              "pointer-events-auto flex max-w-[520px] items-start gap-2.5 rounded-lg border bg-elevated/90 backdrop-blur-md px-3.5 py-2.5 shadow-lg animate-in slide-in-from-bottom-2 fade-in duration-150",
              border[t.kind],
            )}
          >
            {icon[t.kind]}
            {t.onClick ? (
              <button
                type="button"
                className="text-left text-caption text-foreground/90 underline-offset-2 hover:underline"
                onClick={() => {
                  t.onClick?.();
                  onDismiss(t.id);
                }}
              >
                {t.message}
              </button>
            ) : (
              <div className="text-caption text-foreground/90 selectable">
                {t.message}
              </div>
            )}
            <button
              className="ml-1 rounded p-0.5 text-muted-foreground hover:text-foreground"
              onClick={() => onDismiss(t.id)}
              aria-label="Dismiss notification"
            >
              <X aria-hidden className="h-3.5 w-3.5" />
            </button>
          </div>
        ))}
      </div>
    </>
  );
}

export interface RowMenuItem {
  label: string;
  icon?: React.ReactNode;
  onClick: () => void;
  danger?: boolean;
}

/**
 * The ⋯ options menu for list rows. Lives inside the title row so opening it
 * never reflows the metadata line; hidden until the row is hovered or
 * focused, but stays put while open. Clicks stop at the menu so the row's
 * own click handler never fires. Right-clicking the host row (nearest
 * `.group` ancestor) opens it too.
 */
export function RowMenu({
  items,
  label = "Options",
  className,
  onOpen,
  contextItems,
  alwaysVisible = false,
  trigger,
  triggerClassName,
}: {
  items: RowMenuItem[];
  label?: string;
  className?: string;
  /** Fires when the menu opens — hosts use it to dismiss hover cards,
   *  which never get their mouseleave once a menu/dialog takes the pointer. */
  onOpen?: () => void;
  /** Keep the trigger visible at rest instead of revealing it on hover.
   *  For a menu that sits inline in a row (rather than floating over one),
   *  appearing on hover reflows everything beside it. */
  alwaysVisible?: boolean;
  /** Called on right-click, before the menu opens. Return a replacement
   *  item set (the multi-select batch verbs) to show instead of `items`,
   *  or null/undefined to open the normal menu — side effects here (like
   *  collapsing the selection to this row) are welcome (RFC-multi-select). */
  contextItems?: () => RowMenuItem[] | null | undefined;
  /** Replace the ⋯ glyph with custom trigger content (a text link, say).
   *  It stays the SAME button, so the dropdown still anchors to it —
   *  hiding the ⋯ and clicking it from outside gives the menu a zero-sized
   *  trigger rect, which lands it in the window's top-left corner. */
  trigger?: React.ReactNode;
  /** Classes for the trigger button when `trigger` supplies its own look. */
  triggerClassName?: string;
}) {
  const [open, setOpen] = React.useState(false);
  const ref = React.useRef<HTMLDivElement>(null);
  const menuRef = React.useRef<HTMLDivElement>(null);
  const triggerRef = React.useRef<HTMLButtonElement>(null);
  // The menu renders in a body portal with fixed coordinates: host rows
  // wrap their content in stacking contexts (and panels clip at rounded
  // borders), so an in-row absolute menu keeps losing paint-order fights.
  // Fixed-in-portal escapes every ancestor context and clip.
  const [pos, setPos] = React.useState<React.CSSProperties | null>(null);
  // Right-click opens at the cursor (Finder-style) rather than the ⋯
  // trigger, and may swap in the batch item set for a multi-selection.
  const [ctxPos, setCtxPos] = React.useState<{ x: number; y: number } | null>(
    null,
  );
  const [swapItems, setSwapItems] = React.useState<RowMenuItem[] | null>(null);
  // The contextmenu listener is attached once; the callback closes over
  // per-render state (the current selection), so it rides a ref.
  const contextItemsRef = React.useRef(contextItems);
  contextItemsRef.current = contextItems;

  React.useLayoutEffect(() => {
    if (!open || !menuRef.current) {
      if (!open) setPos(null);
      return;
    }
    const m = menuRef.current.getBoundingClientRect();
    if (ctxPos) {
      // Anchor at the cursor; flip up / clamp right at the viewport edges.
      const style: React.CSSProperties = {
        left: Math.max(8, Math.min(ctxPos.x, window.innerWidth - m.width - 8)),
        top:
          ctxPos.y + m.height > window.innerHeight - 8
            ? Math.max(8, ctxPos.y - m.height)
            : ctxPos.y,
      };
      setPos(style);
      return;
    }
    if (!triggerRef.current) return;
    // A display:none trigger measures 0×0 at the origin; fall back to the
    // container so the menu still lands on its row.
    let t = triggerRef.current.getBoundingClientRect();
    if (t.width === 0 && t.height === 0 && ref.current)
      t = ref.current.getBoundingClientRect();
    const up = t.bottom + 4 + m.height > window.innerHeight - 8;
    const style: React.CSSProperties = up
      ? { bottom: window.innerHeight - t.top + 4 }
      : { top: t.bottom + 4 };
    // Right-align to the trigger; open rightwards when that would clip.
    const left = t.right - m.width;
    style.left =
      left < 8 ? Math.min(t.left, window.innerWidth - m.width - 8) : left;
    setPos(style);
  }, [open, ctxPos]);

  // Closing forgets the right-click context — the ⋯ trigger reopens the
  // normal menu at the trigger.
  React.useEffect(() => {
    if (!open) {
      setCtxPos(null);
      setSwapItems(null);
    }
  }, [open]);

  // A fixed menu detaches from its trigger on scroll — close instead.
  React.useEffect(() => {
    if (!open) return;
    const onScroll = () => setOpen(false);
    window.addEventListener("scroll", onScroll, true);
    return () => window.removeEventListener("scroll", onScroll, true);
  }, [open]);

  // Right-clicking anywhere on the host row (the nearest `.group` ancestor)
  // opens the same menu as the ⋯ trigger, replacing the webview's own
  // context menu on rows.
  React.useEffect(() => {
    const row = ref.current?.closest(".group");
    if (!(row instanceof HTMLElement)) return;
    const onContextMenu = (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setSwapItems(contextItemsRef.current?.() ?? null);
      setCtxPos({ x: e.clientX, y: e.clientY });
      setOpen(true);
    };
    row.addEventListener("contextmenu", onContextMenu);
    return () => row.removeEventListener("contextmenu", onContextMenu);
  }, []);

  // One notification point for every way the menu opens (button, context
  // menu, keyboard).
  React.useEffect(() => {
    if (open) onOpen?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  React.useEffect(() => {
    if (!open) return;
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    // Capture-phase pointerdown: title-bar drag regions swallow clicks, but
    // pointerdown still dispatches first. Blur covers leaving the app.
    const onDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (ref.current?.contains(target) || menuRef.current?.contains(target))
        return;
      setOpen(false);
    };
    const onBlur = () => setOpen(false);
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("blur", onBlur);
    };
  }, [open]);

  const focusMenuItem = (direction: 1 | -1) => {
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
    );
    if (items.length === 0) return;
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = current < 0 ? 0 : (current + direction + items.length) % items.length;
    items[next]?.focus();
  };

  const closeAndRestoreFocus = () => {
    // Focus the trigger before the menu unmounts: once focus falls to <body>
    // the container loses group-focus-within, goes display:none, and the
    // trigger becomes unfocusable.
    triggerRef.current?.focus();
    setOpen(false);
  };

  return (
    <div
      ref={ref}
      className={cn(
        "relative shrink-0",
        className,
        open || alwaysVisible
          ? "flex"
          : "hidden group-hover:flex group-focus-within:flex",
      )}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") {
          e.preventDefault();
          closeAndRestoreFocus();
        } else if (e.key === "ArrowDown") {
          e.preventDefault();
          focusMenuItem(1);
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          focusMenuItem(-1);
        } else if (e.key === "Home") {
          e.preventDefault();
          menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
        } else if (e.key === "End") {
          e.preventDefault();
          const items = Array.from(
            menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
          );
          items[items.length - 1]?.focus();
        } else if (e.key === "Tab") {
          setOpen(false);
        }
      }}
    >
      <button
        ref={triggerRef}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (!open && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
            e.preventDefault();
            setOpen(true);
          }
        }}
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        className={
          triggerClassName ??
          "rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
        }
      >
        {trigger ?? <MoreHorizontal className="h-3.5 w-3.5" />}
      </button>
      {open &&
        createPortal(
          <div
            ref={menuRef}
            role="menu"
            data-overlay=""
            aria-label={label}
            style={pos ?? { top: 0, left: 0, visibility: "hidden" }}
            className="menu-glass fixed z-50 w-44 overflow-hidden rounded-md py-1 shadow-[0_0_0_0.5px_var(--border-strong),0_8px_24px_-6px_rgba(0,0,0,0.4)]"
          >
          {(swapItems ?? items).map((it) => (
            <button
              key={it.label}
              role="menuitem"
              onClick={() => {
                closeAndRestoreFocus();
                it.onClick();
              }}
              className={cn(
                "flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-body",
                it.danger
                  ? "text-destructive hover:bg-destructive/10"
                  : "text-foreground/90 hover:bg-surface-2 hover:text-foreground",
              )}
            >
              {it.icon && (
                <span
                  className={it.danger ? undefined : "text-muted-foreground"}
                >
                  {it.icon}
                </span>
              )}
              {it.label}
            </button>
          ))}
          </div>,
          document.body,
        )}
    </div>
  );
}

export function Badge({
  children,
  className,
  title,
}: {
  children: React.ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <span
      title={title}
      className={cn(
        "inline-flex items-center rounded px-1.5 h-[18px] text-micro font-medium",
        "bg-surface-2 text-muted-foreground border border-border",
        className,
      )}
    >
      {children}
    </span>
  );
}

/** True only once `active` has held for `delayMs`. Gates loading indicators
 *  so fast loads render nothing at all — a spinner that flashes for 100ms
 *  reads as a glitch, while one that appears at 250ms reads as "this is a
 *  big file" (PDFs, large repos). */
export function useDelayedFlag(active: boolean, delayMs = 250): boolean {
  const [shown, setShown] = React.useState(false);
  React.useEffect(() => {
    if (!active) {
      setShown(false);
      return;
    }
    const t = window.setTimeout(() => setShown(true), delayMs);
    return () => window.clearTimeout(t);
  }, [active, delayMs]);
  return shown;
}

/** EmptyState's twin for the moment before an answer exists. Same rhythm and
 *  the same slot in a pane, so a list that is still loading doesn't reflow
 *  into its empty state the instant data lands. */
export function LoadingState({
  label,
  compact = false,
}: {
  label: string;
  compact?: boolean;
}) {
  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center gap-2 text-center",
        compact ? "px-4 py-3" : "px-6 py-10",
      )}
      role="status"
    >
      <Spinner className="text-subtle-foreground" />
      <div className="text-caption text-muted-foreground">{label}</div>
    </div>
  );
}

/** The live progress of a retrieval pipeline: completed stages tick off, the
 *  one still running spins, and a transient line (`waiting`) sits below them
 *  without joining the trail. Shared by notebook chat (`chat://step`) and the
 *  Home / ⌘K corpus chat (`meta://step`). */
export function StepTrail({
  steps,
  waiting,
  done,
}: {
  steps: string[];
  waiting: string;
  /** The answer has started arriving — nothing is pending any more. */
  done: boolean;
}) {
  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border bg-surface/60 px-3 py-2">
      {steps.map((s, i) => {
        // The countdown, when there is one, is the thing still running — the
        // last completed step hands its spinner over to it.
        const isLast = i === steps.length - 1 && !waiting;
        const spinning = isLast && !done;
        return (
          <div key={i} className="flex items-center gap-2 text-caption">
            {spinning ? (
              <span
                className="h-2.5 w-2.5 shrink-0 rounded-full border-[1.5px] border-primary border-t-transparent animate-spin"
                aria-hidden
              />
            ) : (
              <Check className="h-3 w-3 shrink-0 text-success" />
            )}
            <span
              className={cn(
                spinning ? "text-foreground" : "text-muted-foreground",
              )}
            >
              {s}
            </span>
          </div>
        );
      })}
      {waiting && !done && (
        <div className="flex items-center gap-2 text-caption" aria-live="polite">
          <span
            className="h-2.5 w-2.5 shrink-0 rounded-full border-[1.5px] border-primary border-t-transparent animate-spin"
            aria-hidden
          />
          <span className="text-muted-foreground">{waiting}</span>
        </div>
      )}
    </div>
  );
}

export function EmptyState({
  icon,
  title,
  hint,
  children,
  compact = false,
}: {
  icon?: React.ReactNode;
  title: string;
  hint?: string;
  children?: React.ReactNode;
  /** Inline section variant: tight vertical rhythm, no icon emphasis. */
  compact?: boolean;
}) {
  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center text-center",
        compact ? "gap-1 px-4 py-3" : "gap-2 px-6 py-10",
      )}
    >
      {icon && <div className="text-subtle-foreground mb-1">{icon}</div>}
      <div className="text-body font-medium text-foreground">{title}</div>
      {hint && (
        <div className="text-caption text-muted-foreground max-w-[260px]">
          {hint}
        </div>
      )}
      {children}
    </div>
  );
}
