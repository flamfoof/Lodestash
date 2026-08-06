<script lang="ts">
  import Icon from "./Icon.svelte";
  import { TABS } from "./routes";
  import { view } from "./nav.svelte";
  import type { LibraryStatus } from "./api";

  let { engineReady, status = null }: { engineReady: boolean; status?: LibraryStatus | null } = $props();

  /** Tab id → live count (UI-SPEC §0.3). Falls back to 0 so a missing payload renders counts of 0. */
  let tabCounts = $derived(new Map((status?.tab_counts ?? []).map((t) => [t.id, t.count])));
</script>

<header class="topbar">
  <div class="brand">
    <Icon name="ls-logo" size={22} />
    <span class="wordmark">LODESTASH</span>
  </div>

  <div class="search" class:ai={view.searchMode === "ai"}>
    <Icon name={view.searchMode === "ai" ? "ls-search-ai" : "ls-search-tags"} size={16} />
    <input
      type="text"
      placeholder={view.searchMode === "ai"
        ? "describe what you need — e.g. »pickup sound for rocks« — then Enter"
        : "Search assets, tags, packs…"}
      value={view.search}
      oninput={(e) => view.setSearch(e.currentTarget.value)}
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
      {@const count = tabCounts.get(tab.id) ?? 0}
      <!-- UI-SPEC §0.3: `All` never hides; a bucket with a zero count hides — unless it is the
           active tab, which must stay visible or the grid silently filters by an invisible tab. -->
      {#if tab.id === "all" || count > 0 || view.tab === tab.id}
        <button
          class:on={view.tab === tab.id}
          style="--tab: var({tab.token})"
          onclick={() => (view.tab = tab.id)}
          data-testid="tab-{tab.id}"
        >
          <Icon name={tab.icon} size={14} />
          {tab.label}
          <span class="count num">{count.toLocaleString()}</span>
        </button>
      {/if}
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
    max-width: 400px;
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
  /* DESIGN-GUIDELINES §7: the active mode colours the input's focus ring — gold for Tags,
     arcane-teal for AI — so you always know which engine you're querying. */
  .search.ai:focus-within {
    border-color: var(--ls-arcane);
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
  /* UI-SPEC §0.3: the active tab carries a 2px underline in its bucket token colour. */
  .tabs button.on {
    box-shadow: inset 0 -2px 0 var(--tab, var(--ls-gold));
  }
  .tabs .count {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .tabs button.on .count {
    color: var(--ls-text-muted);
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
