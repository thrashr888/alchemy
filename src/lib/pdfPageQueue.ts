/** PDF rasterization is expensive. Only two requests may run at once, and
 * pages scrolled past before their turn never reach the backend. */
let active = 0;
const waiting: Array<() => void> = [];

export function queuePdfPage(
  render: () => Promise<string>,
  signal: AbortSignal,
): Promise<string> {
  return new Promise((resolve, reject) => {
    const abort = () => {
      const index = waiting.indexOf(start);
      if (index !== -1) waiting.splice(index, 1);
      reject(new DOMException("Page no longer visible", "AbortError"));
    };
    const start = () => {
      if (signal.aborted) { abort(); return; }
      active++;
      void Promise.resolve().then(() => {
        if (signal.aborted) throw new DOMException("Page no longer visible", "AbortError");
        return render();
      }).then(resolve, reject).finally(() => {
        signal.removeEventListener("abort", abort);
        active--;
        waiting.shift()?.();
      });
    };
    if (signal.aborted) { abort(); return; }
    signal.addEventListener("abort", abort, { once: true });
    if (active < 2) start();
    else waiting.push(start);
  });
}
