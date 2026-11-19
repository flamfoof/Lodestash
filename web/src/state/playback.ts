// What is playing: one sound at a time (UI-SPEC §13.2).
//
// The shape only. The player that owns an <audio> element and advances `position` is `state/player`
// (spec §9.1) and arrives with real playback; until then whoever owns the page hands a `Playback`
// to the components that show one, and they report what was asked (a `play` intent, a seek).

export interface Playback {
  /** The asset whose sound this is. */
  id: number;
  playing: boolean;
  /** Seconds from the start. */
  position: number;
}
