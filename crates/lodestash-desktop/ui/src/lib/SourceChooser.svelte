<script lang="ts">
  /**
   * "Add a source" chooser (design README §"Add a source chooser"): the two ways a folder becomes
   * a library — watch loose assets, or import Asset Store packages (§5.2). The actions live in
   * App.svelte; this is only the picker.
   */
  import Icon from "./Icon.svelte";

  let {
    onwatch,
    onimport,
    onclose,
  }: {
    onwatch: () => void;
    onimport: () => void;
    onclose: () => void;
  } = $props();

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      onclose();
      e.preventDefault();
    }
  }
</script>

<svelte:window on:keydown={onkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={onclose} data-testid="source-chooser">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="sheet"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    aria-labelledby="source-chooser-title"
  >
    <h2 id="source-chooser-title">Add a source</h2>
    <button class="option" onclick={onwatch} data-testid="chooser-watch">
      <Icon name="ls-folder" size={20} />
      <span class="body">
        <span class="label">Watch a folder</span>
        <span class="desc">Index loose assets (FBX / PNG / WAV…) straight from disk.</span>
      </span>
    </button>
    <button class="option" onclick={onimport} data-testid="chooser-import">
      <Icon name="ls-unitypackage" size={20} />
      <span class="body">
        <span class="label">Unity Asset Store packages</span>
        <span class="desc">Scan a folder of .unitypackage files and import them as libraries.</span>
      </span>
    </button>
    <button class="cancel" onclick={onclose}>Cancel</button>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.6);
    display: grid;
    place-items: center;
    z-index: 100;
  }
  .sheet {
    width: min(440px, 92vw);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    padding: var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  h2 {
    font-size: var(--fs-lg);
    margin: 0;
    color: var(--ls-text);
  }
  .option {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-3);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text);
    text-align: left;
    cursor: pointer;
  }
  .option:hover {
    border-color: var(--ls-gold);
    background: var(--ls-surface-raised);
  }
  .option :global(.icon) {
    color: var(--ls-gold);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .label {
    font-weight: 600;
    font-size: var(--fs-sm);
  }
  .desc {
    color: var(--ls-text-dim);
    font-size: var(--fs-xs);
  }
  .cancel {
    align-self: flex-end;
    padding: var(--sp-2) var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    cursor: pointer;
  }
  button:focus-visible {
    outline: 2px solid var(--ls-gold);
    outline-offset: 2px;
  }
</style>
