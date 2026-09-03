/**
 * Provider marks for the cloud sync roots Alchemy detects (DESIGN.md §2, §4).
 *
 * One generic cloud glyph on every row tells a Dropbox mount from a Box mount
 * only by reading the label, so each provider gets its own silhouette:
 * iCloud's plump cloud, OneDrive's wide flat one, Dropbox's two-by-two boxes,
 * Google Drive's segmented triangle, Box's rounded square with the "b".
 *
 * They are simplified silhouettes, not brand assets: monochrome, drawn in
 * `currentColor`, no fills and no brand colors (§2 — color in iconography is
 * reserved for meaning). Nothing loads from the network. Geometry sits on a
 * 16px viewBox at stroke 1.35, which is what lucide's 24px/stroke-2 icons
 * render to at `h-4`, so a mark and a lucide icon carry the same weight in a
 * row.
 */
import { Cloud } from "lucide-react";
import { cn } from "@/lib/utils";

type MarkProps = { className?: string };

function Mark({
  className,
  children,
}: MarkProps & { children: React.ReactNode }) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.35}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={cn("h-4 w-4", className)}
      aria-hidden
    >
      {children}
    </svg>
  );
}

/** iCloud Drive: a tall, round cloud — the plump half of the cloud pair. */
export function ICloudMark({ className }: MarkProps) {
  return (
    <Mark className={className}>
      <path d="M5.2 12.7h5.5a3.05 3.05 0 0 0 .35-6.07 4.15 4.15 0 0 0-7.95-1.1 3.3 3.3 0 0 0 2.1 7.17Z" />
    </Mark>
  );
}

/** OneDrive: a wide cloud on a long flat base — the squat half of the pair. */
export function OneDriveMark({ className }: MarkProps) {
  return (
    <Mark className={className}>
      <path d="M2.6 11.4h9.7a2.5 2.5 0 0 0 .45-4.95 3.35 3.35 0 0 0-5.95-1.5 2.45 2.45 0 0 0-1.95 1.4 2.65 2.65 0 0 0-2.25 5.05Z" />
    </Mark>
  );
}

/** Dropbox: four boxes on the diagonal. */
export function DropboxMark({ className }: MarkProps) {
  return (
    <Mark className={className}>
      <path d="M1 5.2 4.3 2.6 7.6 5.2 4.3 7.8Z" />
      <path d="M8.4 5.2 11.7 2.6 15 5.2 11.7 7.8Z" />
      <path d="M1 10.8 4.3 8.2 7.6 10.8 4.3 13.4Z" />
      <path d="M8.4 10.8 11.7 8.2 15 10.8 11.7 13.4Z" />
    </Mark>
  );
}

/** Google Drive: the triangle, cut into its three panels. The seams are drawn
 *  a hair lighter than the outline — at 16px, three interior lines at full
 *  weight fill the triangle in and it stops reading as a shape. */
export function GoogleDriveMark({ className }: MarkProps) {
  return (
    <Mark className={className}>
      <path d="M8 2.1 14.9 13.7H1.1Z" />
      <path d="M8 9.6V3.6M8 9.6 3.6 12.1M8 9.6l4.4 2.5" strokeWidth={1} />
    </Mark>
  );
}

/** Box: the rounded square around a lowercase b. */
export function BoxMark({ className }: MarkProps) {
  return (
    <Mark className={className}>
      <rect x="1.1" y="1.1" width="13.8" height="13.8" rx="3.6" />
      <path d="M5.5 3.9v7.9" />
      <circle cx="8.55" cy="9.35" r="2.5" />
    </Mark>
  );
}

/** The mark for a provider key from `list_cloud_folders` (or the same key
 *  derived from a path by `folderCloudProvider`). An unrecognized provider
 *  keeps the generic cloud. */
export function CloudMark({
  provider,
  className,
}: MarkProps & { provider: string }) {
  switch (provider) {
    case "icloud":
      return <ICloudMark className={className} />;
    case "onedrive":
      return <OneDriveMark className={className} />;
    case "dropbox":
      return <DropboxMark className={className} />;
    case "google_drive":
      return <GoogleDriveMark className={className} />;
    case "box":
      return <BoxMark className={className} />;
    default:
      return <Cloud className={cn("h-4 w-4", className)} aria-hidden />;
  }
}
