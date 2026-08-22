<script lang="ts">
  import { collections, createCollection, collectionAddAsset } from "./api";
  import { push } from "./toasts.svelte";
  import { view } from "./nav.svelte";

  /**
   * Promote the tray to a Collection (D4, UI-SPEC §6).
   *
   * ★ pins one asset, ⟡ is a cart for right now, ▣ is a set worth naming — each rung can promote to
   * the next. `create_collection` answers with a change count rather than the new row's id, so the
   * id comes from re-listing and matching the name we just wrote; a collision resolves to the newest
   * row, which is the one this click created.
   */
  let naming = $state(false);
  let collName = $state("");
  let saving = $state(false);

  async function saveAsCollection() {
    const name = collName.trim();
    if (!name || saving || ids.length === 0) return;
    saving = true;
    try {
      await createCollection(name);
      const rows = await collections();
      const made = rows.filter((c) => c.name === name).sort((a, b) => b.id - a.id)[0];
      if (!made) throw new Error("the collection did not come back from the catalog");
      for (const id of ids) await collectionAddAsset(made.id, id);
      push(`Saved ${ids.length} to “${name}”`);
      naming = false;
      collName = "";
      view.dataVersion += 1;
    } catch (e) {
      push(String(e), "err");
    } finally {
      saving = false;
    }
  }

  import Icon from "./Icon.svelte";
  import { emptyFilter, gridPage } from "./api";

  let {
    ids,
    onunstage,
    onclear,
    onimport,
  }: {
    /** The staged asset ids, in order (App owns the state; this component just draws it). */
    ids: number[];
    onunstage: (id: number) => void;
    /** `⇢ Add all N to project` (FEATURES §7.2) — queue the whole tray. */
    onimport: (ids: number[]) => void;
    onclear: () => void;
  } = $props();

  /**
   * Resolve staged ids to names. `GridQuery` has **no id filter** (checked: tab / search / status /
   * sort / favorites_only / trashed_only / filter / package / source / collection / links / page /
   * page_size — no `ids`), so a single "give me these ids" query is impossible. We page the full
   * library (name-sorted, bounded to a few pages) and match by id; any id the scanned pages never
   * show is rendered as its raw `#id` rather than a made-up name — honest about what the paging
   * actually covered.
   */
  let names = $state<Map<number, string>>(new Map());
  let resolving = $state(false);

  $effect(() => {
    const want = ids;
    const found = new Map<number, string>();
    const missing = new Set(want);
    if (want.length === 0) {
      names = found;
      resolving = false;
      return;
    }
    resolving = true;
    let stale = false;
    const PAGE = 500;
    const MAX_PAGES = 6;
    (async () => {
      for (let p = 0; p < MAX_PAGES && missing.size > 0; p++) {
        if (stale) return;
        try {
          const r = await gridPage({
            tab: "all",
            search: "",
            search_mode: "tags",
      ids: [],
      status: "all",
            sort_by: "name",
            sort_asc: true,
            favorites_only: false,
            trashed_only: false,
            filter: emptyFilter(),
            package: null,
            collection: null,
            source: null,
            page: p,
            page_size: PAGE,
          });
          for (const it of r.items) {
            if (missing.delete(it.asset_id)) found.set(it.asset_id, it.name);
          }
          if (p + 1 >= r.pages) break;
        } catch {
          break;
        }
      }
      if (!stale) {
        names = found;
        resolving = false;
      }
    })();
    return () => {
      stale = true;
    };
  });

  let rows = $derived(
    ids.map((id) => ({ id, label: names.get(id) ?? null })),
  );
</script>

{#if ids.length > 0}
  <div class="tray num" data-testid="staging-tray" role="region" aria-label="Staging tray">
    <span class="count" data-testid="staged-count">
      <Icon name="ls-action-stage" size={14} />
      {ids.length} staged
    </span>
    <div class="list">
      {#each rows as row (row.id)}
        <span class="item" data-testid="staged-item" title={`asset #${row.id}`}>
          <span class="name">
            {resolving && row.label === null ? "…" : row.label ?? `#${row.id}`}
          </span>
          <button
            class="x"
            data-testid="staged-item-remove"
            aria-label={`Unstage asset #${row.id}`}
            onclick={() => onunstage(row.id)}
          >✕</button>
        </span>
      {/each}
    </div>
    <button class="go" data-testid="tray-import" onclick={() => onimport(ids)} disabled={ids.length === 0}>
      ⇢ Add all {ids.length} to project
    </button>
    <!-- D4: the tray is a cart, a collection is a named set worth keeping. Without this line the
         shortlist someone just built by hand is thrown away on import. One rung up the ladder. -->
    {#if naming}
      <span class="save-as">
        <input
          class="save-name"
          placeholder="Collection name"
          bind:value={collName}
          onkeydown={(e) => e.key === "Enter" && saveAsCollection()}
          data-testid="tray-collection-name"
        />
        <button
          class="clear"
          disabled={saving || !collName.trim()}
          onclick={saveAsCollection}
          data-testid="tray-collection-save"
        >Save</button>
        <button class="clear" onclick={() => (naming = false)}>Cancel</button>
      </span>
    {:else}
      <button
        class="clear"
        disabled={ids.length === 0}
        onclick={() => (naming = true)}
        data-testid="tray-save-collection"
      >▣ Save as collection…</button>
    {/if}
    <button class="clear" data-testid="staged-clear" onclick={onclear}>Clear all</button>
    <span class="spacer"></span>
    <button
      class="commit"
      data-testid="staged-commit"
      disabled
      title="Commit rides the engine build — the Rust/egui collect-and-import pipeline owns the commit side (B9e); this web shell only collects the shortlist for now."
    >
      Commit to project
    </button>
  </div>
{/if}

<style>
  .save-as {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
  }
  .save-name {
    width: 130px;
    height: 24px;
    padding: 0 var(--sp-2);
    font: inherit;
    font-size: var(--fs-xs);
    color: var(--ls-text);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-sm);
  }

  /* A bottom tray (DESIGN-GUIDELINES §7 staging tray): gold hairline on top so it reads as an
     action surface, not another status strip. Gold accent because it is a collect-then-import
     dock, not an AI surface. */
  .tray {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    height: 40px;
    padding: 0 var(--sp-3);
    background: var(--ls-surface);
    border-top: 1px solid var(--ls-gold);
    border-bottom: 1px solid var(--ls-border);
  }
  .count {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    flex: none;
    font-size: var(--fs-sm);
    color: var(--ls-gold);
  }
  .list {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    max-width: 55%;
    overflow-x: auto;
  }
  .item {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    max-width: 180px;
    padding: 2px var(--sp-1) 2px var(--sp-2);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .x {
    display: grid;
    place-items: center;
    flex: none;
    width: 16px;
    height: 16px;
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .x:hover {
    color: var(--ls-danger);
    background: color-mix(in srgb, var(--ls-danger) 15%, transparent);
  }
  .clear {
    flex: none;
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
    text-decoration: underline;
  }
  .clear:hover {
    color: var(--ls-gold);
  }
  .spacer {
    flex: 1;
  }
  .commit {
    flex: none;
    height: var(--control-h);
    padding: 0 var(--sp-3);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    opacity: 0.45;
    cursor: not-allowed;
  }
  .commit:focus-visible {
    box-shadow: var(--el-focus-ring);
  }
</style>
