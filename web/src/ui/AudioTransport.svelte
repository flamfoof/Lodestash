<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";
  import IconButton from "./IconButton.svelte";
  import Slider from "./Slider.svelte";

  /**
   * A sound's transport (UI-SPEC §13.2, §14): play / pause, a scrubber, and the position over the
   * length in mono figures. It plays nothing itself — `playing` and `position` come in, and it
   * reports what was asked (`ontoggle`, `onseek`); whoever owns playback keeps one sound playing
   * at a time. `name` names the sound for the button and the scrubber ("Play rock_pickup_01").
   * `time` formats seconds (the caller's formatter, so the readout matches the stat block).
   * `anchors` marks the parts the contract gate measures.
   */
  let {
    name,
    playing = false,
    position = 0,
    duration,
    time,
    anchors = {},
    ontoggle,
    onseek,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    name: string;
    playing?: boolean;
    /** Seconds from the start. */
    position?: number;
    /** Seconds; 0 when not known yet. */
    duration: number;
    time: (seconds: number) => string;
    anchors?: { play?: string; scrub?: string; time?: string };
    ontoggle?: () => void;
    onseek?: (seconds: number) => void;
  } = $props();

  const at = $derived(Math.min(position, duration));
</script>

<div class="transport" role="group" aria-label="Playback" {...rest}>
  <IconButton
    variant="round"
    size="lg"
    icon={playing ? "pause" : "play"}
    iconSize="14"
    label="{playing ? 'Pause' : 'Play'} {name}"
    onclick={() => ontoggle?.()}
    data-anchor={anchors.play}
  />
  <span class="scrub">
    <Slider
      tone="sound"
      value={at}
      min={0}
      max={duration || 1}
      step={0.05}
      label="Position in {name}"
      valueText="{time(at)} of {time(duration)}"
      disabled={duration <= 0}
      oninput={(seconds) => onseek?.(seconds)}
      data-anchor={anchors.scrub}
    />
  </span>
  <span class="time" data-anchor={anchors.time}>{time(at)} / {time(duration)}</span>
</div>

<style>
  .transport {
    display: flex;
    align-items: center;
    gap: var(--space-12);
    width: 100%;
  }

  /* The slider keeps the 2 px a native range input stands off its neighbours by. */
  .scrub {
    display: flex;
    flex: 1;
    min-width: 0;
    margin: 0 var(--space-2);
  }

  .time {
    flex: none;
    font-family: var(--ff-mono);
    font-size: var(--fs-11);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }
</style>
