import { useEffect, useState, type ReactNode } from "react";
import { DitherBackground } from "./DitherBackground";
import { THEMES, resolveThemeId } from "@/lib/themes";
import { cn } from "@/lib/utils";

// Five minimalist transmutation circles. One is chosen at random per mount and
// they slowly cross-fade (morph) while rotating — Full-Metal-Alchemist-ish.
const SYMBOLS: ReactNode[] = [
  // Squared circle (philosopher's stone)
  <g key="0">
    <circle cx="50" cy="50" r="44" />
    <rect x="19" y="19" width="62" height="62" />
    <path d="M50 12 L84 74 H16 Z" />
    <circle cx="50" cy="50" r="15" />
    <circle cx="50" cy="50" r="2.2" fill="currentColor" stroke="none" />
  </g>,
  // Hexagram
  <g key="1">
    <circle cx="50" cy="50" r="44" />
    <path d="M50 14 L82 68 H18 Z" />
    <path d="M50 86 L18 32 H82 Z" />
    <circle cx="50" cy="50" r="10" />
  </g>,
  // Pentagram
  <g key="2">
    <circle cx="50" cy="50" r="44" />
    <path d="M50 10 L26.5 82.4 L88 37.6 L12 37.6 L73.5 82.4 Z" />
    <circle cx="50" cy="50" r="9" />
  </g>,
  // Transmutation array
  <g key="3">
    <circle cx="50" cy="50" r="44" />
    <circle cx="50" cy="50" r="30" />
    <path d="M50 20 V80 M20 50 H80" />
    <circle cx="50" cy="20" r="5" />
    <circle cx="50" cy="80" r="5" />
    <circle cx="20" cy="50" r="5" />
    <circle cx="80" cy="50" r="5" />
  </g>,
  // Celestial descent
  <g key="4">
    <circle cx="50" cy="50" r="44" />
    <path d="M16 30 H84 L50 88 Z" />
    <circle cx="50" cy="44" r="13" />
    <circle cx="50" cy="26" r="4" />
    <circle cx="50" cy="44" r="2.2" fill="currentColor" stroke="none" />
  </g>,
];

function shuffled<T>(a: T[]): T[] {
  const b = a.slice();
  for (let i = b.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [b[i], b[j]] = [b[j], b[i]];
  }
  return b;
}

export function AlchemySymbol({
  className,
  style,
  preferred,
  strokeWidth = 1,
}: {
  className?: string;
  style?: React.CSSProperties;
  /** Sigil index to open on (a theme's preferred circle); the slow cycle
   *  continues from there. Random start if unset. */
  preferred?: number;
  /** Line weight — notebook-colored contexts use a bolder stroke. */
  strokeWidth?: number;
}) {
  const [order] = useState(() => shuffled(SYMBOLS.map((_, i) => i)));
  const [step, setStep] = useState(() =>
    preferred != null ? Math.max(0, order.indexOf(preferred)) : 0,
  );
  useEffect(() => {
    if (preferred != null) {
      const idx = order.indexOf(preferred);
      if (idx >= 0) setStep(idx);
    }
  }, [preferred, order]);
  useEffect(() => {
    const t = setInterval(() => setStep((s) => (s + 1) % order.length), 9000);
    return () => clearInterval(t);
  }, [order.length]);
  const active = order[step];
  return (
    <div className={cn("relative", className)} style={style}>
      {SYMBOLS.map((s, idx) => (
        <svg
          key={idx}
          viewBox="0 0 100 100"
          fill="none"
          stroke="currentColor"
          strokeWidth={strokeWidth}
          strokeLinejoin="round"
          className="absolute inset-0 h-full w-full transition-opacity duration-[1800ms] ease-in-out"
          style={{ opacity: idx === active ? 1 : 0, animation: "alchemy-spin 90s linear infinite" }}
          aria-hidden
        >
          {s}
        </svg>
      ))}
    </div>
  );
}

/** Dithered atmospheric hero used on blank-slate screens. */
export function AlchemyHero({
  title,
  subtitle,
  epigraph,
  themeKey,
  children,
}: {
  title: string;
  subtitle?: string;
  /** One-line aphorism set beneath the subtitle (see lib/epigraph.ts). */
  epigraph?: string;
  themeKey?: string;
  children?: ReactNode;
}) {
  return (
    <div className="relative isolate flex h-full w-full items-center justify-center overflow-hidden">
      <div className="absolute inset-0">
        <DitherBackground themeKey={themeKey} />
      </div>
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_45%,var(--background)_100%)]" />

      <div className="relative z-10 flex flex-col items-center px-6 text-center">
        <AlchemySymbol
          className="mb-7 h-24 w-24 text-citation/70"
          preferred={THEMES[resolveThemeId(themeKey)]?.sigil}
        />
        <h1 className="font-serif text-5xl font-medium uppercase tracking-[0.22em] text-foreground/90 sm:text-6xl">
          {title}
        </h1>
        {subtitle && (
          <p className="mt-4 max-w-md text-body leading-relaxed text-muted-foreground">
            {subtitle}
          </p>
        )}
        {epigraph && (
          <p className="mt-3 max-w-md font-serif text-body italic leading-relaxed text-subtle-foreground">
            “{epigraph}”
          </p>
        )}
        {children && <div className="mt-8">{children}</div>}
      </div>
    </div>
  );
}
