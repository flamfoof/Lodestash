import { fileUrl } from "./api";

/**
 * The one audio element in the app (UI-SPEC §2.5 / §13.2: *"one asset plays at a time"*).
 *
 * A store rather than per-card state, because "one at a time" is not something 1,841 independent
 * `<audio>` elements can agree on: whoever plays last has to stop whoever played before, and the
 * only way to guarantee that is for there to be exactly one element. Starting a second asset
 * re-points this one — the previous card stops because it has lost the player, not because
 * something remembered to tell it to stop.
 *
 * `lodestash://file` streams the bytes, in-pack payloads included (it materialises the one entry
 * into the size-capped scratch). Over that cap it answers 503, which lands here as `error` — a
 * sound that cannot be pulled says so rather than sitting on a ▶ that never starts.
 */
class Player {
  /** The asset currently loaded, playing or paused. `null` when nothing has been started. */
  current = $state<number | null>(null);
  playing = $state(false);
  /** Seconds. `duration` is 0 until the element has metadata — callers fall back to the catalog. */
  position = $state(0);
  duration = $state(0);
  error = $state<string | null>(null);

  #el: HTMLAudioElement | null = null;

  #element(): HTMLAudioElement {
    if (this.#el) return this.#el;
    const el = new Audio();
    el.preload = "metadata";
    el.addEventListener("timeupdate", () => (this.position = el.currentTime));
    el.addEventListener("loadedmetadata", () => {
      this.duration = Number.isFinite(el.duration) ? el.duration : 0;
    });
    el.addEventListener("play", () => (this.playing = true));
    el.addEventListener("pause", () => (this.playing = false));
    el.addEventListener("ended", () => {
      this.playing = false;
      this.position = 0;
    });
    el.addEventListener("error", () => {
      this.playing = false;
      // The common cause by far is an in-pack payload over the pull cap, and saying "not playable"
      // with no reason is how a user concludes the app is broken.
      this.error = "This sound could not be pulled — it may be inside a pack over the size cap.";
    });
    this.#el = el;
    return el;
  }

  /** Play `id`, or pause it if it is already the one playing. */
  toggle(id: number) {
    const el = this.#element();
    if (this.current === id) {
      if (this.playing) el.pause();
      else void el.play().catch(() => {});
      return;
    }
    // Switching assets: the previous one stops by losing the element, which is the point.
    el.pause();
    this.current = id;
    this.position = 0;
    this.duration = 0;
    this.error = null;
    el.src = fileUrl(id);
    void el.play().catch(() => {});
  }

  /** Seek within the current asset. `frac` is 0–1 of its duration. */
  seek(frac: number) {
    const el = this.#el;
    if (!el || !this.duration) return;
    el.currentTime = Math.max(0, Math.min(1, frac)) * this.duration;
    this.position = el.currentTime;
  }

  stop() {
    this.#el?.pause();
    this.current = null;
    this.playing = false;
    this.position = 0;
  }

  isPlaying(id: number) {
    return this.current === id && this.playing;
  }
}

export const player = new Player();

/** `m:ss.d` — the mono readout beside the scrubber (UI-SPEC §2.5). */
export function fmtClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = seconds - m * 60;
  return `${m}:${s.toFixed(1).padStart(4, "0")}`;
}
