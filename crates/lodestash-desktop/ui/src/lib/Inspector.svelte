<script lang="ts">
  import Icon from "./Icon.svelte";
  import {
    assetDetail,
    previewUrl,
    thumbUrl,
    typeToken,
    typeIcon,
    fmtSize,
    fmtDuration,
    statusLabel,
    bulkTag,
    relatedAssets,
    collections,
    collectionAddAsset,
    collectionRemoveAsset,
    type AssetDetail,
    type RelatedAsset,
    type CollectionRow,
  } from "./api";
  import { view } from "./nav.svelte";
  import { traitColor } from "./traits.svelte";

  let {
    assetId,
    favorite = false,
    trashed = false,
    onfavorite,
    ontrash,
    onrestore,
    onremove,
    ondelete,
  }: {
    assetId: number | null;
    /** Carried by the grid row, so the star reflects the same state the card draws. */
    favorite?: boolean;
    /** True when the selection is in the Trash — the actions change to Restore. */
    trashed?: boolean;
    onfavorite: (on: boolean) => void;
    ontrash: () => void;
    onrestore: () => void;
    onremove: () => void;
    /** Opens the confirm dialog. This panel never deletes anything directly. */
    ondelete: () => void;
  } = $props();

  let detail = $state<AssetDetail | null>(null);
  $effect(() => {
    const id = assetId;
    if (id == null) {
      detail = null;
      return;
    }
    let stale = false;
    assetDetail(id).then((d) => {
      // The panel re-renders on every selection change; a slow reply for a previous
      // selection must not overwrite the current one.
      if (!stale) detail = d;
    });
    return () => {
      stale = true;
    };
  });

  const hideBroken = (e: Event) => ((e.currentTarget as HTMLImageElement).style.display = "none");

  let tint = $derived(detail ? `var(${typeToken(detail.media_type)})` : "var(--ls-text-dim)");

  // ── Tag-editing slice (G4–G9) ──────────────────────────────────────────────────
  // TRAITS (G4): the quick-add write path. The desktop shell has no per-asset tag
  // read (AssetDetail carries no traits), so the chips below are the tags touched in
  // this session — the grid card still draws the asset's real tag set. `bulkTag` with
  // a single id is the same writer the bulk bar (B9d) uses, so the op is undoable.
  let sessionTags = $state<string[]>([]);
  let tagValue = $state("");
  let tagBusy = $state(false);
  let tagMsg = $state<string | null>(null);

  // LINKED FILES (G5) / SIMILAR (G7): both draw the same `related_assets` edges —
  // one fetch, two views of it. `null` while loading so the panels can say so.
  let related = $state<RelatedAsset[] | null>(null);

  // COLLECTIONS (G6): the rows come from `collections()`; membership has no read
  // command in the desktop shell (no `collection_members` wrapper), so it is tracked
  // locally here and toggled through the writer ops. A fresh session cannot know
  // which collections already hold the asset — noted as a gap in the audit report.
  let collectionsRows = $state<CollectionRow[]>([]);
  let membership = $state<Record<number, boolean>>({});

  // Per-asset state resets with the selection. Kept apart from the fetch effect so a
  // `dataVersion` bump (e.g. the collection toggle the user just made) re-asks the
  // data without wiping the local membership guess.
  $effect(() => {
    const id = assetId;
    related = null;
    collectionsRows = [];
    sessionTags = [];
    membership = {};
    if (id == null) return;
  });

  // Fetch the sections' data per selection; re-ask after any catalog write so edges
  // added elsewhere land here too. Stale-guarded like `detail` above.
  $effect(() => {
    const id = assetId;
    view.dataVersion;
    if (id == null) return;
    let stale = false;
    relatedAssets(id)
      .then((r) => {
        if (!stale) related = r;
      })
      .catch(() => {
        if (!stale) related = null;
      });
    collections()
      .then((cs) => {
        if (!stale) collectionsRows = cs;
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  });

  /** Quick-add / remove one `free`-namespace tag on the selected asset. */
  async function applyTag(on: boolean) {
    const id = detail?.asset_id;
    const value = tagValue.trim();
    if (id == null || !value || tagBusy) return;
    tagBusy = true;
    tagMsg = null;
    try {
      await bulkTag([id], "free", value, on);
      sessionTags = on
        ? sessionTags.includes(value)
          ? sessionTags
          : [...sessionTags, value]
        : sessionTags.filter((v) => v !== value);
      tagValue = "";
      view.dataVersion += 1;
    } catch (e) {
      tagMsg = String(e);
    } finally {
      tagBusy = false;
    }
  }

  /** Toggle one collection's membership, tracking state locally (G6). */
  async function toggleCollection(c: CollectionRow) {
    const id = detail?.asset_id;
    if (id == null) return;
    const prev = membership;
    const on = !membership[c.id];
    membership = { ...membership, [c.id]: on };
    try {
      if (on) await collectionAddAsset(c.id, id);
      else await collectionRemoveAsset(c.id, id);
      view.dataVersion += 1;
    } catch (e) {
      membership = prev;
      tagMsg = String(e);
    }
  }

  /** Select a related asset — the same lead selection the grid cards use. */
  function pick(id: number | null) {
    if (id != null) view.selected = id;
  }

  /** Resolved edges show their name; unresolved ones show what was declared. */
  function relLabel(r: RelatedAsset): string {
    return r.name ?? r.declared_path ?? "(unresolved)";
  }
</script>

<aside class="inspector" style="--tint: {tint}">
  {#if !detail}
    <p class="empty">Select an asset.</p>
  {:else}
    <div class="hero" data-testid="inspector-hero">
      {#if detail.has_preview}
        <img src={previewUrl(detail.asset_id)} alt="" />
      {:else if detail.media_type !== "model_3d" && !detail.in_pack}
        <img src={thumbUrl(detail.asset_id)} alt="" onerror={hideBroken} />
      {:else}
        <Icon name={typeIcon(detail.media_type)} size={40} />
      {/if}
    </div>

    <h2 data-testid="inspector-name">{detail.name}</h2>
    <p class="path" title={detail.abs_path}>{detail.rel_path}</p>

    <div class="badges">
      <span class="badge type">{detail.media_type}</span>
      <span class="badge status {detail.status}">{statusLabel(detail.status)}</span>
      {#if detail.in_pack}
        <span class="badge warn" title="Bytes are still inside the .unitypackage">in pack</span>
      {/if}
    </div>

    <dl>
      <dt>Size</dt>
      <dd class="num">{fmtSize(detail.size_bytes)}</dd>
      {#if detail.triangles}
        <dt>Triangles</dt>
        <dd class="num">{detail.triangles.toLocaleString()}</dd>
      {/if}
      {#if detail.width_px && detail.height_px}
        <dt>Dimensions</dt>
        <dd class="num">{detail.width_px} × {detail.height_px}</dd>
      {/if}
      {#if detail.duration_ms}
        <dt>Duration</dt>
        <dd class="num">{fmtDuration(detail.duration_ms)}</dd>
      {/if}
      {#if detail.source}
        <dt>Source</dt>
        <dd>{detail.source}</dd>
      {/if}
    </dl>

    <section class="panel" data-testid="inspector-traits">
      <h3>TRAITS</h3>
      {#if sessionTags.length === 0}
        <p class="hint">No tags edited this session — add one below.</p>
      {:else}
        <div class="tagchips">
          {#each sessionTags as t (t)}
            <span
              class="tag"
              style={traitColor.has(t) ? `--chip: var(${traitColor.get(t)})` : ""}
              title="tagged this session"
            >{t}</span>
          {/each}
        </div>
      {/if}
      <div class="quickadd">
        <input
          bind:value={tagValue}
          placeholder="tag · free"
          aria-label="Tag value"
          data-testid="inspector-tag-input"
          onkeydown={(e) => e.key === "Enter" && applyTag(true)}
        />
        <button
          onclick={() => applyTag(true)}
          disabled={tagBusy || !tagValue.trim()}
          data-testid="inspector-tag-add"
        >+ Add</button>
        <button
          onclick={() => applyTag(false)}
          disabled={tagBusy || !tagValue.trim()}
          data-testid="inspector-tag-remove"
        >− Remove</button>
      </div>
      {#if tagMsg}
        <p class="msg danger">{tagMsg}</p>
      {/if}
    </section>

    <section class="panel" data-testid="inspector-links">
      <h3>LINKED FILES</h3>
      {#if related === null}
        <p class="hint">…</p>
      {:else if related.length === 0}
        <p class="hint">no linked files</p>
      {:else}
        <ul class="links">
          {#each related as r, i (i)}
            <li>
              <button
                class="link"
                onclick={() => pick(r.asset_id)}
                disabled={r.asset_id == null}
                title={`${r.outbound ? "uses" : "used by"} · ${r.kind} · ${r.state}`}
                data-testid="inspector-link-{i}"
              >
                <span class="dir" class:out={r.outbound} aria-hidden="true">{r.outbound ? "→" : "←"}</span>
                <span class="nm">{relLabel(r)}</span>
                {#if r.asset_id == null}<span class="missing">✕ missing</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="panel" data-testid="inspector-collections">
      <h3>COLLECTIONS</h3>
      {#if collectionsRows.length === 0}
        <p class="hint">no collections yet</p>
      {:else}
        <div class="colchips">
          {#each collectionsRows as c (c.id)}
            <button
              class="colchip"
              class:on={membership[c.id]}
              onclick={() => toggleCollection(c)}
              title="{c.asset_count} assets"
              data-testid="collection-toggle-{c.id}"
            >
              {membership[c.id] ? "✓ " : "+ "}{c.name}
            </button>
          {/each}
        </div>
      {/if}
    </section>

    {#if related && related.length > 0}
      <section class="panel" data-testid="inspector-similar">
        <h3>SIMILAR</h3>
        <ul class="links">
          {#each related as r, i (i)}
            <li>
              <button
                class="link"
                onclick={() => pick(r.asset_id)}
                disabled={r.asset_id == null}
                title={`${r.kind} · ${r.state}`}
                data-testid="inspector-similar-{i}"
              >
                <span class="nm">{relLabel(r)}</span>
                {#if r.asset_id == null}<span class="missing">✕ missing</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <!--
      Three removal actions, deliberately not equal-weight (KANBAN B5). Remove is the safe one and
      reads as ordinary; Delete is danger-coloured, last, and opens a confirm dialog. A trashed
      asset shows Restore instead — the Trash has to be a way back, not a second dead end.
    -->
    <div class="actions" data-testid="inspector-actions">
      <button
        class="star"
        class:on={favorite}
        onclick={() => onfavorite(!favorite)}
        data-testid="action-favorite"
        aria-pressed={favorite}
      >
        {favorite ? "★" : "☆"} Favorite
      </button>

      {#if trashed}
        <button onclick={onrestore} data-testid="action-restore">Restore</button>
      {:else}
        <button onclick={ontrash} data-testid="action-trash">Move to Trash</button>
        <button onclick={onremove} data-testid="action-remove" title="Hides it and stops future scans finding it. The file stays on disk.">
          Remove from library
        </button>
      {/if}

      <!--
        No delete for an in-pack asset: it has no file of its own, and the only file behind it is
        the .unitypackage holding every other asset in that pack. The backend refuses this anyway
        (`writer::deletable`), but an offered button that always fails is worse than no button —
        it reads as the app being broken rather than the action being impossible.
      -->
      {#if !detail.in_pack}
        <button class="danger" onclick={ondelete} data-testid="action-delete">
          Delete from disk…
        </button>
      {:else}
        <span class="note">Inside a pack — extract it before it can be deleted individually.</span>
      {/if}
    </div>
  {/if}
</aside>

<style>
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-2);
    margin-top: var(--sp-3);
    padding-top: var(--sp-3);
    border-top: 1px solid var(--ls-border);
  }
  .actions button {
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-sm);
    border: 1px solid var(--ls-border-strong);
    background: var(--ls-surface);
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .actions button:hover {
    border-color: var(--ls-gold);
  }
  .actions .star.on {
    color: var(--ls-gold);
    border-color: var(--ls-gold);
  }
  /* Danger colour and last position: the destructive option should never be the one a hand
     lands on by muscle memory. */
  .actions .danger {
    color: var(--ls-danger);
    border-color: var(--ls-danger);
    margin-left: auto;
  }
  .actions .note {
    margin-left: auto;
    align-self: center;
    max-width: 14rem;
    text-align: right;
    color: var(--ls-text-dim);
    font-size: var(--fs-sm);
  }

  .inspector {
    width: var(--inspector-w);
    flex: none;
    background: var(--ls-surface);
    border-left: 1px solid var(--ls-border);
    padding: var(--sp-4);
    overflow-y: auto;
  }
  .empty {
    color: var(--ls-text-dim);
    margin: 0;
  }

  .hero {
    aspect-ratio: 1;
    display: grid;
    place-items: center;
    background: var(--ls-bg);
    border-radius: var(--r-lg);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--tint) 25%, transparent);
    margin-bottom: var(--sp-3);
    overflow: hidden;
  }
  .hero img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  h2 {
    font-family: var(--ff-ui);
    font-size: var(--fs-lg);
    font-weight: 600;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .path {
    margin: var(--sp-1) 0 var(--sp-3);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    overflow-wrap: anywhere;
    user-select: text;
  }

  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
    margin-bottom: var(--sp-4);
  }
  .badge {
    font-size: var(--fs-xs);
    border-radius: var(--r-pill);
    padding: 1px var(--sp-2);
    border: 1px solid var(--ls-border);
    color: var(--ls-text-muted);
  }
  .badge.type {
    color: var(--tint);
    border-color: color-mix(in srgb, var(--tint) 40%, transparent);
  }
  .badge.status.identified {
    color: var(--ls-status-identified);
  }
  .badge.status.confirmed {
    color: var(--ls-status-confirmed);
  }
  .badge.warn {
    color: var(--ls-warning);
  }

  dl {
    display: grid;
    /* `max-content` on the label and `minmax(0, 1fr)` on the value: without the explicit
       zero minimum a long unbroken token — a pack name like `lowpolycharacterspack_…` —
       sets the column's min-content width and pushes the whole panel past the window edge,
       truncating every other row with it. */
    grid-template-columns: max-content minmax(0, 1fr);
    gap: var(--sp-2) var(--sp-3);
    margin: 0;
    font-size: var(--fs-sm);
  }
  dt {
    color: var(--ls-text-dim);
  }
  dd {
    margin: 0;
    min-width: 0;
    color: var(--ls-text);
    text-align: right;
    overflow-wrap: anywhere;
    user-select: text;
  }

  /* Tag-editing slice (G4–G9): one bordered panel per section, captioned like the
     design's uppercase labels. Inserted between the stat block and the actions, so
     the destructive actions keep their last position. */
  .panel {
    margin-top: var(--sp-4);
    padding-top: var(--sp-3);
    border-top: 1px solid var(--ls-border);
  }
  .panel h3 {
    margin: 0 0 var(--sp-2);
    font-family: var(--ff-ui);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
  }
  .hint {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .msg {
    margin: var(--sp-2) 0 0;
    font-size: var(--fs-xs);
  }
  .msg.danger {
    color: var(--ls-danger);
  }

  /* TRAITS — session chips, styled like the grid card's tag chips. */
  .tagchips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
    margin-bottom: var(--sp-2);
  }
  .tag {
    max-width: 100%;
    padding: 1px var(--sp-2);
    font-size: var(--fs-xs);
    color: var(--chip, var(--ls-text-muted));
    background: var(--ls-overlay);
    border: 1px solid color-mix(in srgb, var(--chip, var(--ls-overlay)) 55%, transparent);
    border-radius: var(--r-pill);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .quickadd {
    display: flex;
    gap: var(--sp-1);
  }
  .quickadd input {
    flex: 1;
    min-width: 0;
    padding: 3px var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .quickadd button {
    flex: none;
    padding: 3px var(--sp-2);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-text);
    background: var(--ls-surface);
    cursor: pointer;
  }
  .quickadd button:hover:not(:disabled) {
    border-color: var(--ls-gold);
  }
  .quickadd button:disabled {
    opacity: 0.45;
    cursor: default;
  }

  /* LINKED FILES / SIMILAR — one row per edge; resolved edges click through to the
     asset, unresolved ones show ✕ missing and are inert. */
  .links {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .link {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    width: 100%;
    padding: 3px var(--sp-2);
    text-align: left;
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-text);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  .link:hover:not(:disabled) {
    border-color: var(--ls-gold);
  }
  .link:disabled {
    cursor: default;
    opacity: 0.75;
  }
  .link .dir {
    flex: none;
    color: var(--ls-text-dim);
  }
  .link .dir.out {
    color: var(--tint);
  }
  .link .nm {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .link .missing {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--ls-danger);
  }

  /* COLLECTIONS — `+ name` / `✓ name` toggle chips, gold when the asset is in. */
  .colchips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
  }
  .colchip {
    max-width: 100%;
    padding: 1px var(--sp-2);
    font: inherit;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    background: none;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .colchip:hover {
    border-color: var(--ls-gold);
  }
  .colchip.on {
    color: var(--ls-gold);
    border-color: var(--ls-gold);
  }
</style>
