/** Own one canvas loop. Hidden surfaces schedule no frames; layout is read
 * only when the surface becomes visible or changes size. */
export function visibleAnimation(
  canvas: HTMLCanvasElement,
  resize: () => void,
  draw: (now: number) => void,
  isStatic: boolean,
): () => void {
  let visible = false;
  let disposed = false;
  let raf = 0;
  let last = -Infinity;
  let needsResize = true;
  let dirty = true;
  const active = () => visible && document.visibilityState !== "hidden" && !disposed;
  const frame = (now: number) => {
    raf = 0;
    if (!active()) return;
    if (dirty || now - last >= 33) {
      if (needsResize) { resize(); needsResize = false; }
      draw(now);
      last = now;
      dirty = false;
    }
    if (!isStatic) raf = requestAnimationFrame(frame);
  };
  const update = () => {
    if (!active()) {
      cancelAnimationFrame(raf);
      raf = 0;
      return;
    }
    if (!raf && (!isStatic || dirty)) raf = requestAnimationFrame(frame);
  };
  const io = new IntersectionObserver(([entry]) => {
    visible = entry.isIntersecting;
    if (visible) { needsResize = true; dirty = true; }
    update();
  });
  const ro = new ResizeObserver(() => {
    needsResize = true;
    dirty = true;
    update();
  });
  const visibility = () => {
    if (document.visibilityState !== "hidden") { needsResize = true; dirty = true; }
    update();
  };
  io.observe(canvas);
  ro.observe(canvas);
  document.addEventListener("visibilitychange", visibility);
  return () => {
    disposed = true;
    cancelAnimationFrame(raf);
    io.disconnect();
    ro.disconnect();
    document.removeEventListener("visibilitychange", visibility);
  };
}
