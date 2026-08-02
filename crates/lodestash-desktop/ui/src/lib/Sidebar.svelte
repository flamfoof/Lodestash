<script lang="ts">
  import Icon from "./Icon.svelte";
  import { NAV } from "./routes";
  import { view } from "./nav.svelte";

  /** Badge counts come from the Engine once it is wired; zero means "no badge". */
  let { counts = {} }: { counts?: Partial<Record<string, number>> } = $props();
</script>

<aside class="sidebar">
  <nav aria-label="Main">
    {#each NAV as item (item.route)}
      {@const badge = item.badge ? (counts[item.badge] ?? 0) : 0}
      <button
        class="row"
        class:on={view.route === item.route}
        aria-current={view.route === item.route ? "page" : undefined}
        onclick={() => view.go(item.route)}
      >
        <Icon name={item.icon} size={18} />
        <span class="label">{item.label}</span>
        {#if badge > 0}
          <span class="badge num">{badge}</span>
        {/if}
      </button>
    {/each}
  </nav>
</aside>

<style>
  .sidebar {
    width: var(--sidebar-w);
    flex: none;
    background: var(--ls-surface);
    border-right: 1px solid var(--ls-border);
    padding: var(--sp-2) var(--sp-2);
    overflow-y: auto;
    position: relative;
    z-index: 1;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    height: var(--row-h);
    padding: 0 var(--sp-2);
    border-radius: var(--r-md);
    /* Active rows get a gold left border (FEATURES §0.3); reserve the space on every
       row so switching rows never shifts the label by 2px. */
    border-left: 2px solid transparent;
    color: var(--ls-text-muted);
    text-align: left;
  }
  .row :global(.icon) {
    opacity: 0.7;
  }
  .row:hover {
    background: var(--ls-overlay);
    color: var(--ls-text);
  }
  .row.on {
    background: var(--ls-surface-raised);
    border-left-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .row.on :global(.icon) {
    opacity: 1;
  }

  .label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .badge {
    font-size: var(--fs-xs);
    color: var(--ls-bg);
    background: var(--ls-gold);
    border-radius: var(--r-pill);
    padding: 1px 7px;
  }
</style>
