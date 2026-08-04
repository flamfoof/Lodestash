<script lang="ts">
  /**
   * Drag-in (DESIGN-GUIDELINES §7 "Drag & drop"): drop a folder on the window to add it as a
   * source and index it. Full-window overlay with a gold dashed border while a drag is over.
   *
   * **A dropped source is indexed, not enriched.** `persist_to_catalog` records path, size, mtime
   * and media type; content hashes, triangle counts, image dimensions and vector previews all come
   * from drains that live in the egui pump (B2b). Until that lands, a folder added here browses
   * and searches but stays out of Duplicates, is excluded by the triangle facet, and shows a type
   * icon rather than a preview for models. The card says so, rather than leaving the user to infer
   * it from a half-populated Inspector.
   *
   * The drop is handled by Tauri's own `onDragDropEvent`, not the DOM's drag events: a WebView
   * `drop` gives you a `File` with no path, and the whole point here is the path — the catalog
   * references originals in place and never copies them (the non-destructive invariant).
   *
   * **The overlay is only for the drag.** Once indexing starts it shrinks to a corner card, because
   * a full-window scrim over a minutes-long scan blocks every click while claiming the window is
   * usable. Browsing is a read on a separate connection, so it genuinely keeps working while the
   * writer thread persists — the UI should not pretend otherwise in either direction.
   *
   * No progress bar: honest progress needs the job tray, which needs the pump (B2b), and a bar
   * that jumps 0 → 100 is a worse lie than a spinner.
   */
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { addSource } from "./api";
  import { view } from "./nav.svelte";

  let { onindexed }: { onindexed: () => void } = $props();

  let over = $state(false);
  let busy = $state<string | null>(null);
  let error = $state<string | null>(null);

  $effect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        if (e.payload.type === "over") over = true;
        else if (e.payload.type === "leave") over = false;
        else if (e.payload.type === "drop") {
          over = false;
          void take(e.payload.paths);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      // Not fatal: the app works, it just cannot be dropped on (a browser dev session has no
      // Tauri webview at all).
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  });

  async function take(paths: string[]) {
    error = null;
    for (const path of paths) {
      busy = path;
      try {
        await addSource(path);
        // Re-ask every screen: the source is new, so nothing on screen knows about it yet.
        view.dataVersion += 1;
        onindexed();
      } catch (e) {
        error = String(e);
      } finally {
        busy = null;
      }
    }
  }
</script>

{#if over}
  <div class="overlay" data-testid="dropzone">
    <div class="panel">
      <p>Drop a folder to add it to your library</p>
      <p class="hint">Files stay where they are — Lodestash only references them.</p>
    </div>
  </div>
{/if}

{#if busy || error}
  <!-- Deliberately not modal: indexing takes minutes and the rest of the app still works. -->
  <div class="toast" class:danger={!!error} data-testid="drop-status" role="status">
    {#if busy}
      <span class="spinner" aria-hidden="true"></span>
      <div>
        <p>Indexing <span class="path">{busy}</span>…</p>
        <p class="hint">
          Large folders take a while. Browsing keeps working; new assets appear when it finishes.
          They start Unidentified, with no duplicate check or model preview until enrichment runs.
        </p>
      </div>
    {:else}
      <div>
        <p data-testid="drop-error">{error}</p>
      </div>
      <button onclick={() => (error = null)}>Dismiss</button>
    {/if}
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 120;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.55);
    border: 3px dashed var(--ls-gold);
  }
  .toast {
    position: fixed;
    right: var(--sp-4);
    bottom: var(--sp-5);
    z-index: 110;
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    max-width: min(420px, 90vw);
    padding: var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.4);
  }
  .toast.danger {
    border-color: var(--ls-danger);
    color: var(--ls-danger);
  }
  .toast .path {
    overflow-wrap: anywhere;
  }
  .panel {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-5);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    text-align: center;
  }
  p {
    margin: 0;
    color: var(--ls-text);
  }
  .hint {
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .danger {
    color: var(--ls-danger);
  }
  .path {
    font-family: var(--font-mono, monospace);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  button {
    padding: var(--sp-1) var(--sp-3);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    background: var(--ls-surface);
    color: var(--ls-text);
    font: inherit;
    cursor: pointer;
  }
  .spinner {
    width: 20px;
    height: 20px;
    border: 2px solid var(--ls-border-strong);
    border-top-color: var(--ls-gold);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  /* Reduced motion: hold the ring still rather than removing the only sign of life. */
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
    }
  }
</style>
