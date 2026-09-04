import { lazy, Suspense } from "react";
import type { RichEditorProps } from "./RichEditor";
import { Spinner } from "./ui";
import { cn } from "@/lib/utils";

const RichEditor = lazy(() =>
  import("./RichEditor").then((m) => ({ default: m.RichEditor })),
);

/**
 * Keeps TipTap and ProseMirror out of the app-start graph. The editor is only
 * needed after a user opens or creates an editable note; its local chunk can
 * load behind the document-sized placeholder without blocking the rest of
 * the workspace.
 */
export function LazyRichEditor(props: RichEditorProps) {
  return (
    <Suspense
      fallback={
        <div
          role="status"
          aria-label="Preparing editor"
          className={cn(
            "flex min-h-[240px] items-center justify-center text-muted-foreground",
            props.fill && "h-full",
            props.bare && "border-0 bg-transparent",
          )}
        >
          <Spinner className="h-4 w-4" />
        </div>
      }
    >
      <RichEditor {...props} />
    </Suspense>
  );
}
