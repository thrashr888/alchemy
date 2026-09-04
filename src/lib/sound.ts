/** Audio cues for work the user explicitly requested, gated by the "Play
 *  sounds" preference. Automatic arrivals, sync, and background jobs stay
 *  silent; their status and errors remain visible in the interface.
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
