<script lang="ts">
  /**
   * Import-packages dialog (FEATURES §5.2): pick a folder → scan for .unitypackage files →
   * list them grouped by containing folder, with per-pack content-category badge + dropdown
   * override, multi-version disclosure, and the §5.6 duplicate flag → "Import as libraries"
   * (one pinned, in-pack-minted package row per ticked pack — nothing copied or unpacked).
   */
  import Icon from "./Icon.svelte";
  import {
    discoverPackageFolder,
    importPackages,
    fmtSize,
    type DiscoveredPackageRow,
  } from "./api";
  import { view } from "./nav.svelte";

  let { root, onclose, onrepick }: {
    root: string;
    onclose: () => void;
    /** Re-pick the folder (the parent owns the native picker + the path). */
    onrepick: () => void;
  } = $props();
  let rows = $state<DiscoveredPackageRow[] | null>(null);
  let error = $state<string | null>(null);
  let scanning = $state(false);
  let importing = $state(false);
  let selected = $state<Set<number>>(new Set());
  let collapsed = $state<Set<string>>(new Set());
  let catFilter = $state<string>("all");
  let overrides = $state<Record<number, string>>({});

  const CATEGORIES = ["3D", "Materials", "Audio", "2D", "Code", "UI"] as const;

  // ── multi-version disclosure (FEATURES §5.2): same-name packs, newest shown ──
  function normName(name: string): string {
    return name.toLowerCase().replace(/[^a-z0-9]/g, "");
  }
  function parseVersion(v: string | null): number[] {
    if (!v) return [];
    return (v.match(/\d+/g) ?? []).map((n) => parseInt(n, 10));
  }
  function cmpVersion(a: number[], b: number[]): number {
    const n = Math.max(a.length, b.length);
    for (let i = 0; i < n; i++) {
      const d = (a[i] ?? 0) - (b[i] ?? 0);
      if (d !== 0) return d;
    }
    return 0;
  }

  let versionFlags = $derived.by<{ newestOf: Map<number, boolean>; olderCount: Map<number, number> }>(() => {
      const list = rows ?? [];
      const byName = new Map<string, number[]>();
      list.forEach((r, i) => {
        const k = normName(r.name);
        const arr = byName.get(k) ?? [];
        arr.push(i);
        byName.set(k, arr);
      });
      const newestOf = new Map<number, boolean>();
      const olderCount = new Map<number, number>();
      for (const idxs of byName.values()) {
        if (idxs.length < 2) {
          for (const i of idxs) newestOf.set(i, true);
          continue;
        }
        const sorted = [...idxs].sort((a, b) =>
          cmpVersion(parseVersion(list[a].version), parseVersion(list[b].version)),
        );
        const newest = sorted[sorted.length - 1];
        for (const i of idxs) newestOf.set(i, i === newest);
        olderCount.set(newest, idxs.length - 1);
      }
      return { newestOf, olderCount };
  });

  function effCategory(r: DiscoveredPackageRow, i: number): string | null {
    return overrides[i] ?? r.category;
  }

  let categories = $derived<string[]>(
    [...new Set((rows ?? []).map((r, i) => effCategory(r, i)).filter((c): c is string => !!c))].sort(),
  );

  let groups = $derived.by<{ group: string; rows: { idx: number; row: DiscoveredPackageRow }[] }[]>(() => {
      const map = new Map<string, { idx: number; row: DiscoveredPackageRow }[]>();
      (rows ?? []).forEach((row, idx) => {
        if (!versionFlags.newestOf.get(idx)) return; // older versions are never listed
        const c = effCategory(row, idx);
        if (catFilter !== "all" && c !== catFilter) return;
        const arr = map.get(row.group) ?? [];
        arr.push({ idx, row });
        map.set(row.group, arr);
      });
      return [...map.entries()]
        .map(([group, rows]) => ({ group, rows }))
        .sort((a, b) => a.group.localeCompare(b.group));
  });

  let selectedCount = $derived(selected.size);

  function toggle(i: number) {
    const next = new Set(selected);
    if (next.has(i)) next.delete(i);
    else next.add(i);
    selected = next;
  }
  function groupOn(group: string): boolean {
    const g = groups.find((x) => x.group === group);
    return !!g && g.rows.length > 0 && g.rows.every(({ idx }) => selected.has(idx));
  }
  function setGroup(group: string, on: boolean) {
    const next = new Set(selected);
    for (const { idx } of groups.find((x) => x.group === group)?.rows ?? []) {
      if (on) next.add(idx);
      else next.delete(idx);
    }
    selected = next;
  }

  async function scan(path: string) {
    scanning = true;
    error = null;
    rows = null;
    selected = new Set();
    overrides = {};
    try {
      rows = await discoverPackageFolder(path);
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
  }

  $effect(() => {
    scan(root);
  });

  function chooseFolder() {
    onrepick();
  }

  async function doImport() {
    if (selectedCount === 0 || importing) return;
    importing = true;
    error = null;
    try {
      const packs = (rows ?? [])
        .map((r, i) => ({ r, i }))
        .filter(({ i }) => selected.has(i))
        .map(({ r, i }) => ({
          abs_path: r.abs_path,
          name: r.name,
          publisher: r.publisher || null,
          category: effCategory(r, i),
        }));
      await importPackages(root, packs);
      view.dataVersion += 1;
      onclose();
    } catch (e) {
      error = String(e);
      importing = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      onclose();
      e.preventDefault();
    }
  }
</script>

<svelte:window on:keydown={onkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={onclose} data-testid="import-dialog">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="sheet"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    aria-labelledby="import-dialog-title"
  >
    <header>
      <Icon name="ls-unitypackage" size={20} />
      <h2 id="import-dialog-title">Import packages</h2>
    </header>

    <div class="folder-row">
      <span class="path" title={root}>{root}</span>
      <button class="ghost" onclick={chooseFolder} data-testid="import-choose-folder">
        Choose folder…
      </button>
    </div>

    {#if scanning}
      <p class="status" data-testid="import-scanning">
        <span class="spinner" aria-hidden="true"></span>
        Scanning for .unitypackage files…
      </p>
    {:else if error && rows === null}
      <p class="status err" data-testid="import-error">{error}</p>
    {:else if rows && rows.length === 0}
      <p class="status" data-testid="import-empty">No .unitypackage files found in this folder.</p>
    {:else if rows}
      <div class="tabs" role="tablist" aria-label="Category filter">
        <button
          class="tab"
          role="tab"
          class:on={catFilter === "all"}
          aria-selected={catFilter === "all"}
          onclick={() => (catFilter = "all")}
        >All</button>
        {#each categories as c (c)}
          <button
            class="tab"
            role="tab"
            class:on={catFilter === c}
            aria-selected={catFilter === c}
            onclick={() => (catFilter = c)}
          >{c}</button>
        {/each}
      </div>

      <div class="list" data-testid="import-list">
        {#each groups as g (g.group)}
          <section class="group">
            <div class="group-head">
              <button
                class="collapse"
                onclick={() => {
                  const next = new Set(collapsed);
                  if (next.has(g.group)) next.delete(g.group);
                  else next.add(g.group);
                  collapsed = next;
                }}
                aria-expanded={!collapsed.has(g.group)}
              >
                <span class="caret" class:open={!collapsed.has(g.group)} aria-hidden="true">▸</span>
                <span class="group-name">{g.group || "Other"}</span>
                <span class="count num">{g.rows.length}</span>
              </button>
              <button class="link" onclick={() => setGroup(g.group, !groupOn(g.group))}>
                {groupOn(g.group) ? "Clear" : "Select all"}
              </button>
            </div>
            {#if !collapsed.has(g.group)}
              <ul class="packs">
                {#each g.rows as { idx, row } (idx)}
                  <li class="pack">
                    <label class="row">
                      <input
                        type="checkbox"
                        checked={selected.has(idx)}
                        onchange={() => toggle(idx)}
                        data-testid="import-check-{idx}"
                      />
                      <span class="pack-body">
                        <span class="name">
                          {row.name}
                          {#if row.duplicate_of}
                            <span class="dup" title="Possibly already indexed">⧉ already indexed as “{row.duplicate_of}”</span>
                          {/if}
                        </span>
                        <span class="meta">
                          {row.version ? "v" + row.version + " · " : ""}{fmtSize(row.size_bytes)} · {row.assets} assets
                        </span>
                      </span>
                      <select
                        class="category"
                        value={effCategory(row, idx) ?? ""}
                        onchange={(e) => {
                          const v = (e.currentTarget as HTMLSelectElement).value;
                          if (v === "") {
                            const next = { ...overrides };
                            delete next[idx];
                            overrides = next;
                          } else {
                            overrides = { ...overrides, [idx]: v };
                          }
                        }}
                        data-testid="import-category-{idx}"
                      >
                        <option value="">{row.category ? "Auto · " + row.category : "Auto-detect"}</option>
                        {#each CATEGORIES as c (c)}
                          <option value={c}>{c}</option>
                        {/each}
                      </select>
                      {#if (versionFlags.olderCount.get(idx) ?? 0) > 0}
                        <span class="older">{(versionFlags.olderCount.get(idx) ?? 0)} older versions · newest shown</span>
                      {/if}
                    </label>
                  </li>
                {/each}
              </ul>
            {/if}
          </section>
        {/each}
      </div>
    {:else}
      <p class="status">Choose a folder to scan for packages.</p>
    {/if}

    {#if error && rows !== null}
      <p class="status err" data-testid="import-error">{error}</p>
    {/if}

    <footer>
      <span class="hint">indexed in place — no files are copied</span>
      <span class="selected num">{selectedCount} selected</span>
      <button
        class="primary"
        onclick={doImport}
        disabled={selectedCount === 0 || importing || scanning}
        data-testid="import-go"
      >
        {importing ? "Importing…" : selectedCount > 0 ? "Import as libraries (" + selectedCount + ")" : "Import as libraries"}
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
    width: min(760px, 94vw);
    max-height: 88vh;
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
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
    color: var(--ls-gold);
  }
  h2 {
    font-size: var(--fs-lg);
    margin: 0;
    color: var(--ls-text);
  }
  .folder-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono, monospace);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .status {
    margin: 0;
    color: var(--ls-text-dim);
    font-size: var(--fs-sm);
  }
  .status.err {
    color: var(--ls-danger);
  }
  .spinner {
    display: inline-block;
    width: 12px;
    height: 12px;
    margin-right: var(--sp-1);
    border: 2px solid var(--ls-border-strong);
    border-top-color: var(--ls-gold);
    border-radius: 50%;
    animation: spin 800ms linear infinite;
    vertical-align: -2px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .tabs {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
  }
  .tab {
    padding: 2px var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    background: none;
    border: 1px solid transparent;
    border-radius: var(--r-pill);
    cursor: pointer;
  }
  .tab.on {
    color: var(--ls-text);
    border-color: var(--ls-gold);
  }
  .list {
    flex: 1;
    min-height: 120px;
    overflow-y: auto;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    background: var(--ls-bg);
  }
  .group-head {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border-bottom: 1px solid var(--ls-border);
    background: var(--ls-surface-raised);
    position: sticky;
    top: 0;
  }
  .collapse {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--ls-text);
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
  }
  .caret {
    display: inline-block;
    transition: transform 120ms ease;
    color: var(--ls-text-dim);
  }
  .caret.open {
    transform: rotate(90deg);
  }
  .group-name {
    font-weight: 600;
    font-size: var(--fs-sm);
  }
  .count {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .link {
    font-size: var(--fs-xs);
    color: var(--ls-gold);
    background: none;
    border: none;
    cursor: pointer;
  }
  .packs {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .pack {
    border-bottom: 1px solid var(--ls-border);
  }
  .pack:last-child {
    border-bottom: none;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2);
    cursor: pointer;
  }
  .row:hover {
    background: var(--ls-overlay);
  }
  .pack-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .name {
    color: var(--ls-text);
    font-size: var(--fs-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dup {
    margin-left: var(--sp-2);
    color: var(--ls-warning);
    font-size: var(--fs-xs);
  }
  .meta {
    color: var(--ls-text-dim);
    font-size: var(--fs-xs);
  }
  .category {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--ls-text);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    padding: 2px var(--sp-1);
  }
  .older {
    flex: none;
    color: var(--ls-text-dim);
    font-size: var(--fs-xs);
  }
  footer {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .hint {
    flex: 1;
    color: var(--ls-text-dim);
    font-size: var(--fs-xs);
  }
  .selected {
    color: var(--ls-text-muted);
    font-size: var(--fs-sm);
  }
  button {
    font: inherit;
    cursor: pointer;
  }
  .ghost {
    padding: var(--sp-1) var(--sp-2);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
  }
  .ghost:hover {
    color: var(--ls-text);
    border-color: var(--ls-gold);
  }
  .primary {
    padding: var(--sp-2) var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--ls-bg);
    background: var(--ls-gold);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-md);
  }
  .primary:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button:focus-visible {
    outline: 2px solid var(--ls-gold);
    outline-offset: 2px;
  }
</style>
