<script lang="ts">
  /**
   * The one dialog in this app that stands in front of an irreversible action.
   *
   * Three rules it exists to enforce (KANBAN B5, owner's call 2026-08-03):
   *
   * 1. It says **permanently**, and names the absolute path. "Delete 1 asset?" tells you nothing
   *    about which file is about to stop existing.
   * 2. Delete is **not** the default. Cancel takes focus, Enter cancels, Escape cancels.
   * 3. The safe alternative lives *in this dialog*. The moment before a delete is the moment
   *    someone discovers that "remove it from my library" was what they actually wanted — a hint
   *    anywhere else in the UI arrives too late to prevent anything.
   */
  import Icon from "./Icon.svelte";

  let {
    names,
    paths,
    onconfirm,
    onremove,
    oncancel,
  }: {
    names: string[];
    /** Absolute paths, so the user can see exactly what leaves the disk. */
    paths: string[];
    onconfirm: () => void;
    onremove: () => void;
    oncancel: () => void;
  } = $props();

  let cancelBtn = $state<HTMLButtonElement | null>(null);
  $effect(() => cancelBtn?.focus());

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      oncancel();
      e.preventDefault();
    }
  }
</script>

<svelte:window on:keydown={onkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={oncancel} data-testid="confirm-delete">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="sheet"
    onclick={(e) => e.stopPropagation()}
    role="alertdialog"
    aria-modal="true"
    tabindex="-1"
    aria-labelledby="confirm-delete-title"
  >
    <header>
      <Icon name="ls-nav-trash" size={20} />
      <h2 id="confirm-delete-title">
        Permanently delete {names.length === 1 ? "this file" : `these ${names.length} files`}?
      </h2>
    </header>

    <p class="lead">
      {names.length === 1 ? "This file" : "These files"} will be
      <strong>erased from your disk</strong>. Lodestash cannot undo this, and
      {names.length === 1 ? "it does" : "they do"} not go to the Recycle Bin.
    </p>

    <ul class="paths" data-testid="confirm-delete-paths">
      {#each paths.slice(0, 8) as p (p)}
        <li>{p}</li>
      {/each}
      {#if paths.length > 8}
        <li class="more">…and {paths.length - 8} more</li>
      {/if}
    </ul>

    <div class="alternative">
      <p>
        <strong>Just want it out of your library?</strong> Remove it instead — the file stays on
        disk, the asset stops showing up here, and re-scanning this folder will not bring it back.
      </p>
      <button class="ghost" onclick={onremove} data-testid="confirm-delete-remove">
        Remove from library
      </button>
    </div>

    <footer>
      <button bind:this={cancelBtn} class="cancel" onclick={oncancel}>Cancel</button>
      <button class="danger" onclick={onconfirm} data-testid="confirm-delete-go">
        Delete from disk
      </button>
    </footer>
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
    width: min(560px, 92vw);
    background: var(--ls-surface);
    border: 1px solid var(--ls-danger);
    border-radius: var(--r-lg);
    padding: var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    color: var(--ls-danger);
  }
  h2 {
    font-size: var(--fs-lg);
    margin: 0;
  }
  .lead {
    margin: 0;
    color: var(--ls-text);
  }
  .paths {
    margin: 0;
    padding: var(--sp-2);
    list-style: none;
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    max-height: 9rem;
    overflow: auto;
    font-family: var(--ff-mono);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
    /* Long paths scroll rather than stretching the dialog off-screen. */
    white-space: nowrap;
  }
  .paths .more {
    font-style: italic;
  }
  .alternative {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-3);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .alternative p {
    margin: 0;
    color: var(--ls-text-dim);
    font-size: var(--fs-sm);
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: var(--sp-2);
  }
  button {
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-md);
    border: 1px solid var(--ls-border-strong);
    background: var(--ls-surface);
    color: var(--ls-text);
    font: inherit;
    cursor: pointer;
  }
  .cancel:focus-visible,
  button:focus-visible {
    outline: 2px solid var(--ls-gold);
    outline-offset: 2px;
  }
  .ghost {
    align-self: flex-start;
  }
  .danger {
    border-color: var(--ls-danger);
    color: var(--ls-danger);
  }
  .danger:hover {
    background: var(--ls-danger);
    color: var(--ls-bg);
  }
</style>
