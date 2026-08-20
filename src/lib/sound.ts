/** The app's audio vocabulary — three event cues, synthesized with WebAudio
 *  (no bundled assets), all gated by the "Play sounds" preference
 *  (Settings → General). Events only, never interactions: sound exists to
 *  re-summon attention that has wandered, not to confirm what the eyes see.
 *
 *  - done:    work the user asked for finished (generation, chat answer)
 *  - arrival: something new appeared on its own (report, folder sync, agent
 *             edits) — only sounds when the window is unfocused, and at most
 *             once per half minute
 *  - error:   something failed — low and distinct, throttled against bursts
 */

let ctx: AudioContext | null = null;

export function soundsEnabled(): boolean {
  return localStorage.getItem("playSounds") !== "false";
}

/** The quiet-while-focused rule, sound edition: with this window focused the
 *  user is already looking, so skip the cue. Mirrored from AiConfig
 *  (quietWhenFocused) into localStorage for this synchronous check — the
 *  backend applies the same rule to desktop notifications. Off = cues play
 *  focused or not. */
function quietNow(): boolean {
  return (
    localStorage.getItem("quietWhenFocused") !== "false" && document.hasFocus()
  );
}

/** One enveloped oscillator note; the building block for every cue. */
function note(
  freq: number,
  start: number,
  dur: number,
  type: OscillatorType,
  peak: number,
) {
  ctx ??= new AudioContext();
  const t = ctx.currentTime + start;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = type;
  osc.frequency.value = freq;
  gain.gain.setValueAtTime(0, t);
  gain.gain.linearRampToValueAtTime(peak, t + 0.02);
  gain.gain.exponentialRampToValueAtTime(0.0001, t + dur);
  osc.connect(gain);
  gain.connect(ctx.destination);
  osc.start(t);
  osc.stop(t + dur + 0.05);
}

/** Settings preview: the done chime with no gates — the user just asked to
 *  hear it, focused or not. */
export function previewSound() {
  try {
    note(660, 0, 0.35, "sine", 0.08);
    note(880, 0.12, 0.35, "sine", 0.08);
  } catch {
    /* audio unavailable */
  }
}

/** Soft two-note completion chime (generation finished, answer done). */
export function playDone() {
  if (!soundsEnabled() || quietNow()) return;
  previewSound();
}

let lastArrival = 0;

/** A single quiet ping: something arrived without being asked for. Silent
 *  while the window is focused — in view, the toast is enough (unless the
 *  quiet-while-focused rule is switched off). */
export function playArrival() {
  if (!soundsEnabled() || quietNow()) return;
  const now = Date.now();
  if (now - lastArrival < 30_000) return;
  lastArrival = now;
  try {
    note(784, 0, 0.5, "sine", 0.05);
  } catch {
    /* audio unavailable */
  }
}

let lastError = 0;

/** Low falling two-note: something failed. Throttled so a burst of related
 *  failures (an error state plus its toast, a failing queue) cues once. */
export function playError() {
  if (!soundsEnabled() || quietNow()) return;
  const now = Date.now();
  if (now - lastError < 5_000) return;
  lastError = now;
  try {
    note(330, 0, 0.3, "triangle", 0.06);
    note(262, 0.11, 0.4, "triangle", 0.06);
  } catch {
    /* audio unavailable */
  }
}
