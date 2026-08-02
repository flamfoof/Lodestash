<script lang="ts">
  import Icon from "./Icon.svelte";
  import { TABS } from "./routes";
  import { view } from "./nav.svelte";

  let { engineReady }: { engineReady: boolean } = $props();
</script>

<header class="topbar">
  <div class="brand">
    <Icon name="ls-logo" size={22} />
    <span class="wordmark">LODESTASH</span>
  </div>

  <div class="search">
    <Icon name="ls-search" size={16} />
    <input
      type="text"
      placeholder="Search assets, tags, packs…"
      bind:value={view.search}
      spellcheck="false"
    />
    <div class="mode" role="group" aria-label="Search mode">
      <button
        class:on={view.searchMode === "tags"}
        onclick={() => (view.searchMode = "tags")}
      >
        <Icon name="ls-search-tags" size={14} /> Tags
      </button>
      <button
        class:on={view.searchMode === "ai"}
        onclick={() => (view.searchMode = "ai")}
      >
        <Icon name="ls-search-ai" size={14} /> AI
      </button>
    </div>
  </div>

  <nav class="tabs" aria-label="Category">
    {#each TABS as tab (tab.id)}
      <button class:on={view.tab === tab.id} onclick={() => (view.tab = tab.id)}>
        <Icon name={tab.icon} size={14} />
        {tab.label}
      </button>
    {/each}
  </nav>

  <div class="chips">
    <span class="chip"><Icon name="ls-project" size={14} /> No project</span>
    <span class="chip" class:warn={!engineReady}>
      {engineReady ? "✦ local" : "✦ shell only"}
    </span>
  </div>
</header>

<style>
  .topbar {
    height: var(--topbar-h);
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-4);
    padding: 0 var(--sp-3);
    background: var(--ls-surface-raised);
    border-bottom: 1px solid var(--ls-border);
    position: relative;
    z-index: 2;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding-right: var(--sp-2);
  }
  .wordmark {
    font-family: var(--ff-display);
    font-weight: 700;
    font-size: var(--fs-lg);
    letter-spacing: 0.08em;
    color: var(--ls-gold);
  }

  .search {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex: 1 1 auto;
    max-width: 560px;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text-dim);
  }
  .search:focus-within {
    border-color: var(--ls-gold);
  }
  .search input {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    outline: none;
    color: var(--ls-text);
    font: inherit;
    user-select: text;
  }
  .search input::placeholder {
    color: var(--ls-text-dim);
  }

  .mode {
    display: flex;
    gap: 2px;
    background: var(--ls-surface);
    border-radius: var(--r-sm);
    padding: 2px;
  }
  .mode button,
  .tabs button {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 3px var(--sp-2);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
    opacity: 0.75;
  }
  .mode button.on,
  .tabs button.on {
    background: var(--ls-overlay);
    color: var(--ls-text);
    opacity: 1;
  }
  .mode button:hover,
  .tabs button:hover {
    color: var(--ls-text);
  }

  .tabs {
    display: flex;
    gap: var(--sp-1);
  }

  .chips {
    margin-left: auto;
    display: flex;
    gap: var(--sp-2);
    align-items: center;
  }
  .chip {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    padding: 3px var(--sp-3);
  }
  .chip.warn {
    color: var(--ls-warning);
    border-color: color-mix(in srgb, var(--ls-warning) 40%, transparent);
  }
</style>
