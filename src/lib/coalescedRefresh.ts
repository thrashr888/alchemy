/** Collapse event bursts into a single read, with at most one read in flight.
 * Events arriving during a read require a trailing read; that older response
 * may not publish. Errors cannot wedge subsequent refreshes. */
export function coalescedRefresh(
  run: (isCurrent: () => boolean) => Promise<void>,
  onError: (error: unknown) => void,
  delay = 150,
) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let running = false;
  let revision = 0;
  let disposed = false;
  const schedule = () => {
    if (disposed || running || timer !== undefined) return;
    timer = setTimeout(() => {
      timer = undefined;
      running = true;
      const started = revision;
      void run(() => !disposed && started === revision)
        .catch(onError)
        .finally(() => {
          running = false;
          if (started !== revision) schedule();
        });
    }, delay);
  };
  return {
    request() { revision++; schedule(); },
    dispose() { disposed = true; clearTimeout(timer); },
  };
}
