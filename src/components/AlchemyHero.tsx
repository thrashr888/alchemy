import type { ReactNode } from "react";
import { DitherBackground } from "./DitherBackground";

/** The alchemical "squared circle" (philosopher's stone): circle · triangle · square · point. */
function AlchemySymbol({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 100 100" className={className} fill="none" aria-hidden>
      <g stroke="currentColor" strokeWidth="1.1" strokeLinejoin="round">
        <circle cx="50" cy="50" r="46" opacity="0.55" />
        <rect x="20.5" y="20.5" width="59" height="59" opacity="0.4" />
        <path d="M50 12 L84 74 H16 Z" opacity="0.7" />
        <circle cx="50" cy="56" r="16" opacity="0.85" />
        <circle cx="50" cy="56" r="2.2" fill="currentColor" stroke="none" />
      </g>
    </svg>
  );
}

/** Dithered atmospheric hero used on blank-slate screens. */
export function AlchemyHero({
  title,
  subtitle,
  themeKey,
  children,
  compact,
}: {
  title: string;
  subtitle?: string;
  themeKey?: string;
  children?: ReactNode;
  compact?: boolean;
}) {
  return (
    <div className="relative isolate flex h-full w-full items-center justify-center overflow-hidden">
      <div className="absolute inset-0">
        <DitherBackground themeKey={themeKey} />
      </div>
      {/* Vignette so text stays legible over the dither. */}
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_30%,var(--background)_90%)]" />

      <div className="relative z-10 flex flex-col items-center px-6 text-center">
        <AlchemySymbol
          className={
            compact
              ? "mb-5 h-14 w-14 text-citation"
              : "mb-7 h-24 w-24 text-citation drop-shadow-[0_0_24px_var(--selection)]"
          }
        />
        <h1
          className={
            compact
              ? "font-serif text-[26px] font-medium tracking-[0.14em] text-foreground"
              : "font-serif text-5xl font-medium uppercase tracking-[0.2em] text-foreground sm:text-6xl"
          }
        >
          {title}
        </h1>
        {subtitle && (
          <p className="mt-4 max-w-md text-[13.5px] leading-relaxed text-muted-foreground">
            {subtitle}
          </p>
        )}
        {children && <div className="mt-7">{children}</div>}
      </div>
    </div>
  );
}
