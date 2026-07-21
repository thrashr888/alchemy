import * as React from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/utils";
import {
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
  sm: "h-7 px-2.5 text-[12px] gap-1.5 rounded-md",
  md: "h-8 px-3 text-[13px] gap-2 rounded-md",
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
        "h-8 w-full rounded-md bg-surface-2 px-2.5 text-[13px] text-foreground",
        "border border-input placeholder:text-subtle-foreground outline-none",
        "focus:border-ring/70 focus:ring-1 focus:ring-ring/40 transition-colors",
        className,
      )}
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
        "w-full rounded-md bg-surface-2 px-2.5 py-2 text-[13px] text-foreground resize-none",
        "border border-input placeholder:text-subtle-foreground outline-none",
        "focus:border-ring/70 focus:ring-1 focus:ring-ring/40 transition-colors",
        className,
      )}
      {...props}
    />
  );
}

export function Spinner({ className }: { className?: string }) {
  return <Loader2 className={cn("animate-spin", className)} />;
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
  onClick: () => void;
  className?: string;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      className={cn(
        "absolute inset-0 z-0 rounded-[inherit] outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
        className,
      )}
    />
  );
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
        "absolute inset-y-0 z-20 w-1.5 cursor-col-resize transition-colors hover:bg-ring/30 active:bg-ring/40 focus-visible:bg-ring/30",
        // Fully inside the card edge: the panels clip at their rounded
        // border (overflow-hidden), so a straddling handle loses its
        // outer half to hit-testing.
        edge === "right" ? "right-0" : "left-0",
      )}
    />
  );
}

let modalSeq = 0;

export function Modal({
  open,
  onClose,
  title,
  children,
  footer,
  headerActions,
  width = "max-w-md",
  tall = false,
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
      window.removeEventListener("keydown", onKey);
      // Restore focus to whatever opened the dialog — one tick later, so
      // the keystroke that closed it (Enter submitting a form) can't land
      // on the refocused trigger and re-activate it.
      const t = window.setTimeout(() => trigger?.focus?.(), 0);
      void t;
    };
  }, [open]);

  if (!open) return null;
  return (
    <div
      className={cn(
        "fixed inset-0 z-50 flex items-start justify-center bg-black/40 backdrop-blur-[2px] animate-in fade-in duration-150",
        tall ? "pt-[6vh]" : "pt-[12vh]",
      )}
      onMouseDown={onClose}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className={cn(
          tall ? "max-h-[88vh]" : "max-h-[80vh]",
          "flex w-full flex-col rounded-lg bg-elevated outline-none animate-in zoom-in-95 duration-150",
          "shadow-[0_0_0_0.5px_var(--border-strong),0_16px_48px_-8px_rgba(0,0,0,0.45)]",
          width,
        )}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex min-h-11 shrink-0 items-center justify-between gap-3 border-b border-border px-4 py-2">
          <h2
            id={titleId}
            className="text-[13px] font-semibold text-foreground"
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
        <div className="min-h-0 flex-1 overflow-y-auto p-4">{children}</div>
        {footer && (
          <div className="shrink-0 border-t border-border px-4 py-3">
            {footer}
          </div>
        )}
      </div>
    </div>
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
    confirmLabel: string;
    danger: boolean;
    resolve: (ok: boolean) => void;
  } | null>(null);

  const confirm = React.useCallback(
    (opts: {
      title: string;
      message?: string;
      confirmLabel?: string;
      danger?: boolean;
    }) =>
      new Promise<boolean>((resolve) => {
        setState({
          title: opts.title,
          message: opts.message ?? "",
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
        <p className="text-[13px] leading-relaxed text-muted-foreground">
          {state.message}
        </p>
      )}
    </Modal>
  ) : null;

  return { confirm, dialog };
}

/** Bottom-center stack of ephemeral toasts. */
export function Toaster({
  toasts,
  onDismiss,
}: {
  toasts: Toast[];
  onDismiss: (id: string) => void;
}) {
  if (toasts.length === 0) return null;
  const icon = {
    success: <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0 text-success" />,
    error: (
      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" />
    ),
    info: <Info className="mt-0.5 h-4 w-4 shrink-0 text-citation" />,
  };
  const border = {
    success: "border-success/40",
    error: "border-destructive/40",
    info: "border-border-strong",
  };
  return (
    <div
      role="status"
      aria-live="polite"
      aria-atomic="false"
      className="pointer-events-none fixed bottom-[calc(1rem+env(safe-area-inset-bottom))] left-1/2 z-[70] flex -translate-x-1/2 flex-col items-center gap-2"
    >
      {toasts.map((t) => (
        <div
          key={t.id}
          className={cn(
            "pointer-events-auto flex max-w-[520px] items-start gap-2.5 rounded-lg border bg-elevated/90 backdrop-blur-md px-3.5 py-2.5 shadow-lg animate-in slide-in-from-bottom-2 fade-in duration-150",
            border[t.kind],
          )}
        >
          {icon[t.kind]}
          <div className="text-[12px] text-foreground/90 selectable">
            {t.message}
          </div>
          <button
            className="ml-1 rounded p-0.5 text-muted-foreground hover:text-foreground"
            onClick={() => onDismiss(t.id)}
            aria-label="Dismiss notification"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      ))}
    </div>
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
}: {
  items: RowMenuItem[];
  label?: string;
  className?: string;
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

  React.useLayoutEffect(() => {
    if (!open || !menuRef.current || !triggerRef.current) {
      if (!open) setPos(null);
      return;
    }
    const t = triggerRef.current.getBoundingClientRect();
    const m = menuRef.current.getBoundingClientRect();
    const up = t.bottom + 4 + m.height > window.innerHeight - 8;
    const style: React.CSSProperties = up
      ? { bottom: window.innerHeight - t.top + 4 }
      : { top: t.bottom + 4 };
    // Right-align to the trigger; open rightwards when that would clip.
    const left = t.right - m.width;
    style.left =
      left < 8 ? Math.min(t.left, window.innerWidth - m.width - 8) : left;
    setPos(style);
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
      setOpen(true);
    };
    row.addEventListener("contextmenu", onContextMenu);
    return () => row.removeEventListener("contextmenu", onContextMenu);
  }, []);

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
        open ? "flex" : "hidden group-hover:flex group-focus-within:flex",
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
        className="rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground"
      >
        <MoreHorizontal className="h-3.5 w-3.5" />
      </button>
      {open &&
        createPortal(
          <div
            ref={menuRef}
            role="menu"
            aria-label={label}
            style={pos ?? { top: 0, left: 0, visibility: "hidden" }}
            className="menu-glass fixed z-50 w-44 overflow-hidden rounded-md py-1 shadow-[0_0_0_0.5px_var(--border-strong),0_8px_24px_-6px_rgba(0,0,0,0.4)]"
          >
          {items.map((it) => (
            <button
              key={it.label}
              role="menuitem"
              onClick={() => {
                closeAndRestoreFocus();
                it.onClick();
              }}
              className={cn(
                "flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-[13px]",
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
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded px-1.5 h-[18px] text-[11px] font-medium",
        "bg-surface-2 text-muted-foreground border border-border",
        className,
      )}
    >
      {children}
    </span>
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
      <div className="text-[13px] font-medium text-foreground">{title}</div>
      {hint && (
        <div className="text-[12px] text-muted-foreground max-w-[260px]">
          {hint}
        </div>
      )}
      {children}
    </div>
  );
}
