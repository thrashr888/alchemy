/** Every caller waits for the same complete boot, including StrictMode's
 * repeated effect. A boolean guard returns too early to the second caller. */
export function initializeOnce(
  initialize: () => Promise<void>,
): () => Promise<void> {
  let pending: Promise<void> | undefined;
  return () => (pending ??= Promise.resolve().then(initialize));
}

/** Called only from committed ready content, after its Suspense boundary
 * resolves. Leave a frame for that content to paint before reporting it. */
export function afterStartupPaint(report: () => void): () => void {
  let second: number | undefined;
  const first = requestAnimationFrame(() => {
    second = requestAnimationFrame(report);
  });
  return () => {
    cancelAnimationFrame(first);
    if (second !== undefined) cancelAnimationFrame(second);
  };
}
