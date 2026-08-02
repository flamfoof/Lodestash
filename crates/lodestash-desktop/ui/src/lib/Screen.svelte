<script lang="ts">
  import type { Snippet } from "svelte";

  /**
   * Stub screen body. Every route renders one of these until its milestone builds the
   * real thing (B3 Library/Favorites, B4 viewers, B5 settings/jobs, B6 the rest), so an
   * unfinished route is obviously unfinished rather than a blank pane.
   */
  let {
    title,
    milestone,
    children,
  }: { title: string; milestone: string; children?: Snippet } = $props();
</script>

<section class="screen">
  <header>
    <h1>{title}</h1>
    <span class="tag">{milestone}</span>
  </header>
  <div class="body">
    {#if children}
      {@render children()}
    {:else}
      <p class="stub">Not built yet — lands in {milestone}.</p>
    {/if}
  </div>
</section>

<style>
  .screen {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: var(--sp-3);
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
    border-bottom: 1px solid var(--ls-border);
  }
  h1 {
    font-size: var(--fs-2xl);
    color: var(--ls-text);
  }
  .tag {
    font-family: var(--ff-mono);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    padding: 1px var(--sp-2);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--sp-5);
  }
  .stub {
    color: var(--ls-text-dim);
    margin: 0;
  }
</style>
