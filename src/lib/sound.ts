/** Work-done chime, synthesized with WebAudio — no bundled asset, and the
 *  "Play sounds" preference (Settings → General) gates it. */

let ctx: AudioContext | null = null;

export function soundsEnabled(): boolean {
  return localStorage.getItem("playSounds") !== "false";
}

/** Soft two-note completion chime (generation finished, answer done). */
export function playDone() {
  if (!soundsEnabled()) return;
  try {
    ctx ??= new AudioContext();
    const t = ctx.currentTime;
    for (const [freq, start] of [
      [660, 0],
      [880, 0.12],
    ] as const) {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = "sine";
      osc.frequency.value = freq;
      gain.gain.setValueAtTime(0, t + start);
      gain.gain.linearRampToValueAtTime(0.08, t + start + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, t + start + 0.35);
      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.start(t + start);
      osc.stop(t + start + 0.4);
    }
  } catch {
    /* audio unavailable */
  }
}
