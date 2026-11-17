<script lang="ts">
  /**
   * The brand: the lodestone — a gold-edged diamond with a bright core and a soft glow — and
   * "LODESTASH" in Cinzel. `textAttrs` reaches the name (the contract measures it).
   */
  let {
    textAttrs = {},
    collapse,
  }: {
    textAttrs?: Record<string, string | undefined>;
    /** In the app bar, keep only the stone below this rung (AppBar). */
    collapse?: "xl" | "lg" | "md";
  } = $props();
</script>

<div class="brand {collapse ? `collapse-${collapse}` : ''}">
  <span class="stone" aria-hidden="true"><span class="core"></span></span>
  <span class="name" {...textAttrs}>LODESTASH</span>
</div>

<style>
  .brand {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    flex-shrink: 0;
  }

  .stone {
    display: flex;
    align-items: center;
    justify-content: center;
    width: var(--space-22);
    height: var(--space-22);
    border: var(--bw-2) solid var(--ls-gold);
    border-radius: var(--r-4);
    background: var(--ls-gold-a15);
    box-shadow: var(--glow-gold-8-a50);
    transform: rotate(45deg);
  }

  .core {
    width: var(--space-5);
    height: var(--space-5);
    border-radius: var(--r-pill);
    background: var(--ls-gold-bright);
  }

  @container appbar (width < 1000px) {
    .collapse-md .name {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
      white-space: nowrap;
    }
  }

  .name {
    font-family: var(--ff-display);
    font-size: var(--fs-16);
    font-weight: var(--fw-700);
    letter-spacing: var(--track-1);
    color: var(--ls-gold);
  }
</style>
