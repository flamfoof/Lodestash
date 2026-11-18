<script lang="ts">
  /**
   * The five-bar mark that stands for a sound where a picture would be (UI-SPEC §2.5): aqua, or
   * slate under a `?` when the sound is Unidentified (`unknown`). A glyph, not the sound's
   * waveform — decoration only.
   */
  let { unknown = false }: { unknown?: boolean } = $props();
</script>

<span class="wave" class:unknown aria-hidden="true">
  <i></i><i></i><i></i><i></i><i></i>
  {#if unknown}<span class="mark">?</span>{/if}
</span>

<style>
  .wave {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-4);
    height: var(--space-30);
  }

  i {
    width: var(--space-5);
    border-radius: var(--r-pill);
    background: var(--ls-type-sound);
  }

  /* The prototype's bar heights (10, 22, 30, 17, 12 px) as shares of the 30 px mark. */
  i:nth-child(1) {
    height: 33%;
  }

  i:nth-child(2) {
    height: 73%;
  }

  i:nth-child(3) {
    height: 100%;
  }

  i:nth-child(4) {
    height: 57%;
  }

  i:nth-child(5) {
    height: 40%;
  }

  .unknown i {
    background: var(--ls-wave-unidentified);
  }

  .mark {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: var(--ff-mono);
    font-size: var(--fs-22);
    color: var(--ls-status-unidentified);
    text-shadow: 0 0 var(--space-10) var(--ls-bg);
  }
</style>
