<script lang="ts">
  /**
   * Ctrl/Cmd-K command palette (ROADMAP P7 "keyboard-first"): one box that reaches both the
   * library and the app's own navigation.
   *
   * Two groups, assets first, because "find the one asset I need" is what this product is for and
   * a palette that makes you scroll past eleven screen names to reach it is a worse search box
   * than the one already in the top bar. Commands are matched on their label only — a fuzzy
   * matcher that scores "Duplicates" against "dup" is a nice-to-have; substring already reaches
   * every command from three letters.
   */
  import { gridPage, type GridItem } from "./api";
  import { view } from "./nav.svelte";
  import { NAV, TABS } from "./routes";

  let { onclose }: { onclose: () => void } = $props();

  type Command = { label: string; hint: string; run: () => void };

  const COMMANDS: Command[] = [
    ...NAV.map((n) => ({
      label: n.label,
      hint: "Go to",
      run: () => view.go(n.route),
    })),
    ...TABS.map((t) => ({
      label: t.label,
      hint: "Category",
      run: () => {
        view.go("library");
        view.tab = t.id;
      },
    })),
    { label: "Grid view", hint: "View", run: () => { view.browse = "flat"; view.rowStyle = "grid"; } },
    { label: "List view", hint: "View", run: () => { view.browse = "flat"; view.rowStyle = "list"; } },
    { label: "Folders view", hint: "View", run: () => { view.browse = "folders"; view.rowStyle = "grid"; } },
    { label: "Toggle inspector", hint: "View", run: () => (view.inspectorOpen = !view.inspectorOpen) },
    { label: "Clear filters", hint: "Filter", run: () => view.clearFilter() },
  ];

  let q = $state("");
  let assets = $state<GridItem[]>([]);
  let active = $state(0);
  let box = $state<HTMLInputElement | null>(null);

  $effect(() => box?.focus());

  let commands = $derived(
    q.trim() === ""
      ? COMMANDS.slice(0, 8)
      : COMMANDS.filter((c) => c.label.toLowerCase().includes(q.trim().toLowerCase())).slice(0, 8),
  );

  // Debounced: the palette queries on every keystroke and the grid behind it is 8k rows, so an
  // un-debounced box runs a whole-library sort per character typed.
  $effect(() => {
    const needle = q.trim();
    if (needle === "") {
      assets = [];
      return;
    }
    const t = setTimeout(() => {
      gridPage({
        tab: "all",
        search: needle,
      search_mode: "tags",
      status: "all",
        sort_by: "relevance",
        sort_asc: true,
        favorites_only: false,
        trashed_only: false,
        filter: { traits: [], tri_buckets: [], sizes: [] },
        page: 0,
        page_size: 8,
      })
        .then((r) => (assets = r.items))
        .catch(() => (assets = []));
    }, 120);
    return () => clearTimeout(t);
  });

  // One flat list, so the arrow keys do not have to know about the group boundary.
  type Row = { kind: "asset"; item: GridItem } | { kind: "command"; command: Command };
  let rows = $derived<Row[]>([
    ...assets.map((item) => ({ kind: "asset" as const, item })),
    ...commands.map((command) => ({ kind: "command" as const, command })),
  ]);

  $effect(() => {
    // Keep the cursor on a row that still exists as results change under it.
    if (active >= rows.length) active = 0;
  });

  function run(row: Row) {
    if (row.kind === "command") {
      row.command.run();
    } else {
      // Reveal it in the library rather than opening it: the grid, the Inspector and Quick Look
      // are all one keystroke away once the asset is selected and on screen.
      view.go("library");
      view.setSearch(row.item.name);
      view.selected = row.item.asset_id;
    }
    onclose();
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      onclose();
      e.preventDefault();
    } else if (e.key === "ArrowDown") {
      active = rows.length === 0 ? 0 : (active + 1) % rows.length;
      e.preventDefault();
    } else if (e.key === "ArrowUp") {
      active = rows.length === 0 ? 0 : (active - 1 + rows.length) % rows.length;
      e.preventDefault();
    } else if (e.key === "Enter" && rows[active]) {
      run(rows[active]);
      e.preventDefault();
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={onclose} data-testid="palette">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="sheet" onclick={(e) => e.stopPropagation()}>
    <input
      bind:this={box}
      bind:value={q}
      {onkeydown}
      placeholder="Search assets, or jump to a screen"
      data-testid="palette-input"
      aria-label="Command palette"
    />

    {#if rows.length === 0}
      <p class="msg">No matches.</p>
    {:else}
      <ul data-testid="palette-results">
        {#each rows as row, i (row.kind === "asset" ? `a${row.item.asset_id}` : `c${row.command.label}`)}
          <li>
            <button
              class="row"
              class:active={i === active}
              onclick={() => run(row)}
              onmouseenter={() => (active = i)}
            >
              {#if row.kind === "asset"}
                <span class="name">{row.item.name}</span>
                <span class="hint">{row.item.media_type}</span>
              {:else}
                <span class="name">{row.command.label}</span>
                <span class="hint">{row.command.hint}</span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.5);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    z-index: 90;
  }
  .sheet {
    width: min(560px, 92vw);
    background: var(--ls-surface-raised, var(--ls-surface));
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    box-shadow: 0 16px 48px rgb(0 0 0 / 0.5);
    overflow: hidden;
  }
  input {
    width: 100%;
    padding: var(--sp-3);
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--ls-border);
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-lg);
  }
  input:focus {
    outline: none;
  }
  ul {
    margin: 0;
    padding: var(--sp-1);
    list-style: none;
    max-height: 50vh;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    padding: var(--sp-2);
    border-radius: var(--r-sm);
    background: none;
    border: none;
    color: var(--ls-text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row.active {
    background: var(--ls-overlay);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .msg {
    margin: 0;
    padding: var(--sp-3);
    color: var(--ls-text-dim);
  }
</style>
