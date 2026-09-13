import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/api";
import { queuePdfPage } from "@/lib/pdfPageQueue";
import { Spinner } from "./ui";

/** Keep bitmaps only around the viewport. Remember each page's aspect ratio
 * after decoding, so releasing a landscape page does not move the scrollbar. */
export function PdfPage({ path, page, count, title, width }: {
  path: string; page: number; count: number; title: string; width: number;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(false);
  const [image, setImage] = useState<{ url: string; width: number } | null>(null);
  const [ratio, setRatio] = useState(8.5 / 11);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const node = root.current;
    if (!node) return;
    const observer = new IntersectionObserver(([entry]) => setNear(entry.isIntersecting), {
      rootMargin: "600px",
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    setImage(null);
    setFailed(false);
    if (!near || !path || width <= 0) return;
    const controller = new AbortController();
    // Let resize bursts settle before making another full bitmap.
    const timer = window.setTimeout(() => {
      void queuePdfPage(() => api.pdfPageImage(path, page, width), controller.signal)
        .then((url) => {
          if (!controller.signal.aborted) setImage({ url, width });
        }).catch(() => {
          if (!controller.signal.aborted) setFailed(true);
        });
    }, 100);
    return () => { window.clearTimeout(timer); controller.abort(); };
  }, [near, path, page, width]);

  return (
    <div ref={root} className="w-full" style={{ maxWidth: width || undefined }}>
      {near && image?.width === width ? (
        <img src={image.url} alt={`${title} — page ${page}`}
          onLoad={(event) => {
            const img = event.currentTarget;
            if (img.naturalWidth && img.naturalHeight) setRatio(img.naturalWidth / img.naturalHeight);
          }}
          className="w-full rounded-md border border-border shadow-sm" />
      ) : (
        <div className="flex w-full items-center justify-center rounded-md border border-border bg-surface-2/40"
          style={{ aspectRatio: ratio }}>
          {near && (failed ? <span className="text-caption text-muted-foreground">This page could not be rendered.</span> : <Spinner className="h-4 w-4" />)}
        </div>
      )}
      <div className="pt-1.5 text-center text-micro text-subtle-foreground">{page} / {count}</div>
    </div>
  );
}
