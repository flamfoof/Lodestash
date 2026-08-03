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
    type AssetDetail,
  } from "./api";

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
</style>
