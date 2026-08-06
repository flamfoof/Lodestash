<script lang="ts">
  import { getContext } from "svelte";
  import Icon from "./Icon.svelte";
  import { type GridItem, previewUrl, thumbUrl, typeToken, typeIcon, fmtSize, stage } from "./api";
  import { traitColor } from "./traits.svelte";

  let {
    item,
    selected = false,
    onselect,
    onopen,
  }: {
    item: GridItem;
    selected?: boolean;
    onselect?: (id: number) => void;
    onopen?: (id: number) => void;
  } = $props();

  /**
   * Stage affordance (I6): the app shell owns the staging state and exposes it via Svelte context
   * (App.svelte `setContext("ls:staging", …)`), so a card can toggle membership without every
   * intermediate screen threading a callback through its props. Rendered outside the shell (no
   * context), the card stages directly — the button stays alive, the tray just isn't wired up.
   */
  const staging = getContext<{
    isStaged?: (id: number) => boolean;
    toggleStage?: (id: number) => void;
  }>("ls:staging");

  let staged = $derived(staging?.isStaged?.(item.asset_id) ?? false);

  function onStageClick() {
    if (staging?.toggleStage) staging.toggleStage(item.asset_id);
    else void stage([item.asset_id]);
  }

  let tint = $derived(`var(${typeToken(item.media_type)})`);
  // Vector preview if the model has one, else a raster thumbnail, else the type mark. `thumbFailed`
  // covers the cases the backend cannot render (in-pack, exotic formats) without a second request.
  let thumbFailed = $state(false);
  // Raster thumbnails are for assets that *have* pixels. A `model_3d` thumbnail costs a full FBX
  // parse per card — 100 of those is why the grid must not ask for one; models get the cheap
  // vector preview from the drain instead, and show their type mark until it arrives.
  //
  // In-pack assets *are* asked for: the route pulls the one payload into the thumbnail cache when
  // the archive is small enough, and answers 503 when it is not — which `thumbFailed` turns into
  // the type mark with no second request.
  let canRaster = $derived(item.media_type !== "model_3d" && !thumbFailed);
  let src = $derived(
    item.has_preview ? previewUrl(item.asset_id) : canRaster ? thumbUrl(item.asset_id) : null,
  );
  let unidentified = $derived(item.status === "discovered" || item.status === "indexed");

  // FEATURES §1.1: the meta line leads with the pack, because "which pack is this from" is the
  // question a grid of 8k near-identical Synty props actually raises. Size moved to the tier chip.
  let sub = $derived(
    [
      item.package,
      item.triangles ? `${item.triangles.toLocaleString()} tris` : null,
      fmtSize(item.size_bytes),
    ]
      .filter(Boolean)
      .join(" · "),
  );

  // Three chips then a count. Showing every trait makes card height depend on how well-tagged an
  // asset happens to be, which turns the grid ragged for no gain.
  const SHOWN = 3;
  let chips = $derived(item.traits.slice(0, SHOWN));
  let overflow = $derived(Math.max(0, item.traits.length - SHOWN));
</script>

<button
  class="card"
  class:selected
  style="--tint: {tint}"
  onclick={() => onselect?.(item.asset_id)}
  ondblclick={() => onopen?.(item.asset_id)}
  title={item.rel_path}
>
  <div class="thumb">
    {#if src}
      <!-- Vector previews render natively as SVG; rasters come from the on-disk thumb cache. -->
      <img {src} alt="" loading="lazy" onerror={() => (thumbFailed = true)} />
    {:else}
      <Icon name={typeIcon(item.media_type)} size={28} />
    {/if}
    <span class="corner">
      {#if item.favorite}
        <span class="star" title="Favourite" aria-label="Favourite">★</span>
      {/if}
      <span
        class="state"
        class:unknown={unidentified}
        title={unidentified ? "Unidentified" : item.status}
      >
        {unidentified ? "?" : "✓"}
      </span>
    </span>
    {#if item.size_tier}
      <span class="tier num t{item.size_tier}">{item.size_tier} · {fmtSize(item.size_bytes)}</span>
    {/if}
    <!-- The stage affordance (I6) is a hover-revealed quick action, bottom-right on the artwork so
         it never collides with the corner's status gem or the tier badge. A span, not a button:
         the card root is already a <button>, and nesting a real button inside one is invalid HTML. -->
    <span
      class="stage-btn"
      class:on={staged}
      role="button"
      tabindex="0"
      title={staged ? "Staged — click to remove from the tray" : "Stage for import"}
      aria-label={staged ? `Unstage ${item.name}` : `Stage ${item.name}`}
      aria-pressed={staged}
      data-testid="card-stage"
      onclick={(e) => {
        e.stopPropagation();
        onStageClick();
      }}
      onkeydown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          e.stopPropagation();
          onStageClick();
        }
      }}
    >⧉</span>
  </div>
  <div class="meta">
    <span class="name"><i class="bullet" aria-hidden="true"></i>{item.name}</span>
    <span class="sub num">{sub}</span>
  </div>
  <div class="tags">
    {#if unidentified}
      <!-- The tag row is where a reader looks for what an asset *is*; an unidentified one says so
           there rather than hiding a marker on the artwork. -->
      <span class="unknown-tags">???</span>
    {:else}
      {#each chips as t (t)}
        <span
          class="tag"
          style={traitColor.has(t) ? `--chip: var(${traitColor.get(t)})` : ""}
        >{t}<i class="dash" aria-hidden="true"></i></span>
      {/each}
      {#if overflow > 0}
        <span class="tag more" title={item.traits.slice(SHOWN).join(", ")}>+{overflow}</span>
      {/if}
    {/if}
  </div>
</button>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-lg);
    text-align: left;
    overflow: hidden;
    /* A <button> computes width: auto as fit-content, not fill — so a card sized to its
       content instead of its grid track: measured 224px next to a 428px card in the same
       row (long unbroken names like SM_Env_Entrance_Crypt_01_Collision), overflowing the
       cell and making the gaps ragged. width: 100% pins every card to its track; the
       ellipsizing name/sub lines clip inside it. */
    width: 100%;
    min-width: 0;
  }
  .card:hover {
    border-color: var(--tint);
    background: var(--ls-surface-raised);
  }
  .card.selected {
    border-color: var(--ls-gold);
    box-shadow: 0 0 0 1px var(--ls-gold);
  }

  .thumb {
    position: relative;
    aspect-ratio: 1;
    display: grid;
    place-items: center;
    background: var(--ls-bg);
    border-radius: var(--r-md);
    /* The type colour reads through the preview as a faint wash, so a card's category is
       legible at a glance without tinting the artwork itself. */
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--tint) 22%, transparent);
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  /* Star and status share the top-right corner (FEATURES §1.1) so the artwork's top-left stays
     clear for the multi-select box a later slice adds. */
  .corner {
    position: absolute;
    top: var(--sp-1);
    right: var(--sp-1);
    display: flex;
    align-items: center;
    gap: var(--sp-1);
  }
  .state {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    font-size: var(--fs-xs);
    line-height: 1;
    color: var(--ls-status-identified);
    background: color-mix(in srgb, var(--ls-bg) 80%, transparent);
    border-radius: var(--r-pill);
  }
  .state.unknown {
    color: var(--ls-status-unidentified);
  }

  /* The size badge sits *on* the artwork, bottom-left, so the letter reads against a constant
     backdrop rather than whatever colour the meta row happens to be. */
  .tier {
    position: absolute;
    left: var(--sp-1);
    bottom: var(--sp-1);
    padding: 1px 6px;
    font-size: var(--fs-xs);
    line-height: 1.4;
    color: var(--tier-fg, var(--ls-text-muted));
    background: color-mix(in srgb, var(--ls-bg) 82%, transparent);
    border: 1px solid color-mix(in srgb, var(--tier-fg, var(--ls-border)) 40%, transparent);
    border-radius: var(--r-sm);
  }
  .tS {
    --tier-fg: var(--ls-success);
  }
  .tM {
    --tier-fg: var(--ls-info);
  }
  .tL {
    --tier-fg: var(--ls-gold);
  }
  .tXL {
    --tier-fg: var(--ls-danger);
  }

  /* Hover-revealed stage quick action (I6). Sits bottom-right on the artwork, opposite the tier
     badge, so the two never overlap; the card's hover also reveals it (hovering the button itself
     keeps `.card:hover` true). */
  .stage-btn {
    position: absolute;
    right: var(--sp-1);
    bottom: var(--sp-1);
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    font-size: var(--fs-sm);
    line-height: 1;
    color: var(--ls-gold);
    background: color-mix(in srgb, var(--ls-bg) 85%, transparent);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    opacity: 0;
    transition: opacity 120ms ease;
  }
  .card:hover .stage-btn,
  .stage-btn:focus-visible {
    opacity: 1;
  }
  .stage-btn:hover {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }
  .stage-btn.on {
    opacity: 1;
    border-color: var(--ls-gold);
    background: var(--ls-overlay);
  }

  .star {
    font-size: var(--fs-sm);
    line-height: 1;
    color: var(--ls-gold);
    text-shadow: 0 1px 2px var(--ls-bg);
  }

  .meta {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-sm);
    color: var(--ls-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Rotated square, not a glyph: the media-type mark has to read at 7px, and every dingbat that
     renders as a diamond in one font falls back to a box in another. */
  .bullet {
    flex: none;
    width: 7px;
    height: 7px;
    background: var(--tint);
    transform: rotate(45deg);
  }
  .sub {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Fixed height for one row of chips: card height must not depend on how well-tagged an asset
     is, or the grid goes ragged and every row reflows as tags land. */
  .tags {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    height: 18px;
    overflow: hidden;
  }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 100%;
    padding: 1px 6px;
    font-size: var(--fs-xs);
    line-height: 1.4;
    color: var(--chip, var(--ls-text-muted));
    background: var(--ls-overlay);
    border: 1px solid color-mix(in srgb, var(--chip, var(--ls-overlay)) 55%, transparent);
    border-radius: var(--r-pill);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Per-trait chip colour (B8a): a styled trait sets `--chip`; unstyled chips keep the muted
     default. The dash stays the type colour either way — one glance still says type + trait. */
  .dash {
    flex: none;
    width: 10px;
    height: 2px;
    background: var(--tint);
    border-radius: var(--r-pill);
  }
  .more {
    color: var(--ls-text-dim);
  }
  .unknown-tags {
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    color: var(--ls-status-unidentified);
  }
</style>
