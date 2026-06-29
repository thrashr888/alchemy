import * as React from "react";
import { cn } from "@/lib/utils";
import { Loader2, X } from "lucide-react";

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
  sm: "h-7 px-2.5 text-[12.5px] gap-1.5 rounded-md",
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
}) {
  return (
    <button
      className={cn(
        "inline-flex items-center font-medium transition-colors select-none outline-none",
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
}: React.InputHTMLAttributes<HTMLInputElement>) {
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
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
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

export function Modal({
  open,
  onClose,
  title,
  children,
  width = "max-w-md",
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: React.ReactNode;
  width?: string;
}) {
  React.useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/60 backdrop-blur-[2px] pt-[12vh] animate-in fade-in duration-150"
      onMouseDown={onClose}
    >
      <div
        className={cn(
          "flex max-h-[80vh] w-full flex-col rounded-lg border border-border-strong bg-elevated shadow-2xl animate-in zoom-in-95 duration-150",
          width,
        )}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex shrink-0 items-center justify-between border-b border-border px-4 h-11">
          <h2 className="text-[13px] font-semibold text-foreground">{title}</h2>
          <Button variant="ghost" size="icon" onClick={onClose}>
            <X className="h-4 w-4" />
          </Button>
        </div>
        <div className="overflow-y-auto p-4">{children}</div>
      </div>
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
        "inline-flex items-center rounded px-1.5 h-[18px] text-[10.5px] font-medium",
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
}: {
  icon?: React.ReactNode;
  title: string;
  hint?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center text-center gap-2 px-6 py-10">
      {icon && <div className="text-subtle-foreground mb-1">{icon}</div>}
      <div className="text-[13px] font-medium text-foreground">{title}</div>
      {hint && <div className="text-[12px] text-muted-foreground max-w-[260px]">{hint}</div>}
      {children}
    </div>
  );
}
