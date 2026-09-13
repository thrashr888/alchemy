import { useEffect, useRef, useState } from "react";
import { api } from "@/lib/api";
import { PdfPage } from "./PdfPage";

/** PDF pages release their bitmaps outside the nearby viewport. */
export function PdfPageView({
  sourceId,
  title,
}: {
  sourceId: string;
  title: string;
}) {
  /** Resolved local path. A file source is already local; a URL source is
   *  downloaded into the cache on first open (commands::pdf_local_path). */
  const [path, setPath] = useState("");
  const [count, setCount] = useState(0);
  const [failed, setFailed] = useState(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // One width for every page, measured once per resize: pages in a PDF share
  // a page size almost always, and re-rendering each on its own measurement
  // would thrash PDFium for no visible gain.
  const [width, setWidth] = useState(0);

  useEffect(() => {
    let stale = false;
    setPath("");
    setCount(0);
    setFailed(false);
    api
      .pdfLocalPath(sourceId)
      .then((p) => !stale && setPath(p))
      .catch(() => !stale && setFailed(true));
    return () => {
      stale = true;
    };
  }, [sourceId]);

  useEffect(() => {
    if (!path) return;
    let stale = false;
    api
      .pdfPageCount(path)
      .then((n) => {
        if (stale) return;
        setCount(n);
        if (n === 0) setFailed(true);
      })
      .catch(() => !stale && setFailed(true));
    return () => {
      stale = true;
    };
  }, [path]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () =>
      setWidth(Math.round(Math.min(el.clientWidth - 48, 1100)));
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [count]);

  if (failed) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center p-6">
        <span className="text-body text-muted-foreground">
          The pages could not be rendered — the file may have moved.
        </span>
      </div>
    );
  }

  return (
    <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto p-6">
      <div className="mx-auto flex flex-col items-center gap-6">
        {Array.from({ length: count }, (_, i) => i + 1).map((page) => (
          <PdfPage key={`${path}:${page}`} path={path} page={page} count={count}
            title={title} width={width} />
        ))}
      </div>
    </div>
  );
}

