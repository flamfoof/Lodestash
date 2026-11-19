<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A sound's waveform as a row of rounded bars (UI-SPEC §14: Quick Look's 30-bar waveform). Each
   * peak is a bar's height as a share of the 130 px band; the bars sit on its floor, centred.
   * `played` (0–1) colours the part already heard — bright against the resting tone, so progress
   * reads as a boundary and not as a colour alone. Leave it out before playback has started.
   *
   * A picture of the sound, not a control: the transport beside it carries the position, so this
   * is hidden from assistive technology. `barAttrs` reaches the first and last bars (the gate).
   */
  let {
    peaks,
    played,
    barAttrs = {},
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    peaks: readonly number[];
    played?: number;
    barAttrs?: { first?: Record<`data-${string}`, string | undefined>; last?: Record<`data-${string}`, string | undefined> };
  } = $props();

  const heard = (i: number) => played !== undefined && i / peaks.length <= played;
</script>

<div class="wave" aria-hidden="true" {...rest}>
  {#each peaks as peak, i (i)}
    <i class:heard={heard(i)} style:--peak={peak} {...i === 0 ? barAttrs.first : i === peaks.length - 1 ? barAttrs.last : {}}></i>
  {/each}
</div>

<style>
  .wave {
    display: flex;
    align-items: flex-end;
    justify-content: center;
    gap: var(--space-3);
    width: 100%;
    height: var(--wave-h);
  }

  i {
    width: var(--space-8);
    height: calc(var(--peak) * 100%);
    flex: none;
    border-radius: var(--r-pill);
    background: var(--ls-wave-rest);
    transition: background var(--dur-100) linear;
  }

  .heard {
    background: var(--ls-arcane-bright);
  }
</style>
