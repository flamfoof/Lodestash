<script lang="ts">
  import { getContext } from "svelte";
  import Icon from "./Icon.svelte";
  import {
    type GridItem,
    previewUrl,
    thumbUrl,
    typeToken,
    typeIcon,
    fmtSize,
    stage,
    identifyAsset,
  } from "./api";
  import { traitColor } from "./traits.svelte";
  import { sources } from "./sources.svelte";
  import { player, fmtClock } from "./audio.svelte";

  let {
    item,
    selected = false,
    focusable = false,
    onselect,
    onopen,
    onidentified,
    onmenu,
    onfavorite,
    onpack,
    ontrait,
  }: {
    item: GridItem;
    selected?: boolean;
    /**
     * Click. `range` is true when Shift was held — the grid extends the pick set from the lead to
     * this card rather than moving the lead (D15).
     */
    onselect?: (id: number, range?: boolean) => void;
    /**
     * D15 roving `tabindex`: the grid is ONE tab stop and arrows move inside it. Without this, a
     * focus ring on cards means Tab lands on all 50–100 of them before it reaches anything else.
     */
    focusable?: boolean;
    onopen?: (id: number) => void;
    /** After a ✦ identify completes (B11b) — lets the grid bump its page so the new
     *  preview/tags render. */
    onidentified?: (id: number) => void;
    /** Right-click (FEATURES §1.1): the shell's context menu. The Card only reports the
     *  event + asset — the menu itself lives up-tree where the write fns are. */
    onmenu?: (e: MouseEvent, id: number) => void;
    /**
     * Pack name → Pack stats (UI-SPEC §2.5: "pack name is a link → Pack stats §15"). Reported up
     * rather than opened here: the modal is one per screen, not one per card.
     */
    onpack?: (name: string) => void;
    /**
     * Trait chip → filter by it (§2.5: "chips are click-to-search"). The chip answers the question
     * it raises — *show me the rest of these* — and lands in the active-filter bar, where it is
     * removable like every other axis (D2).
     */
    ontrait?: (value: string) => void;
    /** ☆ → ★ (FEATURES §1.1 "star tile" — UI-SPEC §2.5's overlay list does not carry it): the
     *  star is a control, not a read-out. Reports the *current*
     *  flag; the shell inverts it, exactly as the context menu's Toggle Favorite does. */
    onfavorite?: (id: number, favorite: boolean) => void;
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

  // ✦ Identify now (B11b): one bounded appraise pass on the persist thread. The card stays
  // interactive; the ✦ spins until the kick resolves, then the grid bumps its page so the
  // new preview/tags appear.
  let identifying = $state(false);
  async function runIdentify() {
    if (identifying) return;
    identifying = true;
    try {
      await identifyAsset(item.asset_id);
      onidentified?.(item.asset_id);
    } catch (e) {
      console.error("identify failed", e);
    } finally {
      identifying = false;
    }
  }

  function onStageClick() {
    if (staging?.toggleStage) staging.toggleStage(item.asset_id);
    else void stage([item.asset_id]);
  }

  /**
   * The source this row belongs to is not on disk right now (UI-SPEC §11.1).
   *
   * The catalog keeps every fact it holds — name, pack, tags, tier, stars, imported badge all
   * still read, which is the entire point of an index. Only the two things that need the *bytes*
   * change: the artwork becomes a stated plate instead of a broken image, and ⟡ goes inert,
   * because staging a file the import would skip is a promise the tray cannot keep.
   *
   * ⤤ deliberately stays live and is the one useful thing left: it opens Quick Look's
   * "isn't reachable" panel, which carries the path and **Relink**. (UI-SPEC §11.1 said both went
   * inert; an inert ⤤ beside a live double-click on the same card is a contradiction, and it would
   * leave the panel the same section specifies reachable only by accident. Bullet amended there.)
   */
  let unreachable = $derived(sources.unavailable(item.source_id));

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
  let confirmed = $derived(item.status === "confirmed");
  // The corner gem carries identification status (DESIGN-GUIDELINES §6): the sprite's three
  // `ls-status-*` gems — `?` grey, ✓ verdant, ★ gold for confirmed (a human reviewed it).
  let gem = $derived(
    unidentified ? "ls-status-unidentified" : confirmed ? "ls-status-confirmed" : "ls-status-identified",
  );

  /**
   * UI-SPEC §2.5 sound cards: a ▶ button + a **5-bar** mini-waveform stands in for the thumbnail,
   * because an audio file has no pixels and one shared speaker glyph makes 1,841 sounds render as
   * 1,841 identical dark squares.
   *
   * The bar heights are hashed off the asset id rather than the real envelope: decoding audio per
   * card to draw 5 bars is absurd, and a *stable, per-asset* silhouette is what the grid actually
   * needs — the same card looks the same on every visit, and neighbours look different.
   * ponytail: hashed silhouette, not real audio; swap in peaks if the drain ever stores them.
   */
  let isSound = $derived(item.media_type === "audio");
  /**
   * The card’s own transport (UI-SPEC §2.5). Only one asset plays at a time, so a card asks the
   * shared player whether it is the one — it holds no playback state of its own, and a card that
   * scrolls out of view and back cannot disagree with what is actually coming out of the speakers.
   */
  let isCurrent = $derived(isSound && player.current === item.asset_id);
  let isPlaying = $derived(isSound && player.isPlaying(item.asset_id));
  /** Catalog duration is the fallback until the element has metadata of its own. */
  let clipSeconds = $derived(
    isCurrent && player.duration ? player.duration : (item.duration_ms ?? 0) / 1000,
  );
  let progress = $derived(
    isCurrent && clipSeconds ? Math.min(1, player.position / clipSeconds) : 0,
  );

  function toggleAudio() {
    player.toggle(item.asset_id);
  }

  /** Click anywhere on the bar to seek there — the scrubber is the bar, not a separate control. */
  function scrubFrom(e: MouseEvent) {
    const el = e.currentTarget as HTMLElement;
    const r = el.getBoundingClientRect();
    player.seek((e.clientX - r.left) / Math.max(1, r.width));
  }
  let bars = $derived.by(() => {
    // `Math.imul`, not `*`: the product of two ~2^31 values passes `Number.MAX_SAFE_INTEGER`, and
    // the float rounding that follows zeroes the low bits the next `>>> 15` depends on — so the
    // heights would come off a coarse lattice and neighbouring cards would repeat silhouettes.
    let h = Math.imul(item.asset_id, 2654435761);
    return Array.from({ length: 5 }, () => {
      h = Math.imul(h ^ (h >>> 15), 2246822519);
      return 35 + (Math.abs(h) % 66); // 35–100% of the strip
    });
  });

  /**
   * Tileable art (texture/material) fills the frame; a subject (image/sprite/font) is letterboxed.
   * Cropping a tiling albedo costs nothing — cropping a character sprite takes its head off.
   */
  let coverFit = $derived(item.media_type === "texture" || item.media_type === "material");

  // FEATURES §1.1: the meta line leads with the pack, because "which pack is this from" is the
  // question a grid of 8k near-identical Synty props actually raises. Size moved to the tier chip.
  //
  // The pack is rendered separately from the rest rather than joined into one string — that string
  // is exactly why §2.5’s "pack name is a link" was never built: there was nothing to attach it to.
  const fmtTris = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k` : `${n}`);
  let stats = $derived(
    [item.triangles ? `${fmtTris(item.triangles)} tris` : null, fmtSize(item.size_bytes)]
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
  class:identified={!unidentified}
  class:confirmed
  class:unidentified
  class:selected
  style="--tint: {tint}"
  data-asset-id={item.asset_id}
  tabindex={focusable ? 0 : -1}
  onclick={(e) => onselect?.(item.asset_id, e.shiftKey)}
  oncontextmenu={(e) => {
    e.preventDefault();
    onmenu?.(e, item.asset_id);
  }}
  ondblclick={() => onopen?.(item.asset_id)}
  title={item.rel_path}
>
  <div class="thumb" class:unid={unidentified} class:unreachable>
    {#if unreachable}
      <!-- Stated, not broken (UI-SPEC §11.1). A missing drive that renders as a failed <img>
           reads as "Lodestash is broken"; this reads as "the drive is out", which is true and
           actionable. -->
      <span class="gone-plate" data-testid="card-unavailable">
        <span class="gone-mark" aria-hidden="true">✕</span>
        <span class="gone-text">file unavailable</span>
      </span>
    {:else if src}
      <!-- Vector previews render natively as SVG; rasters come from the on-disk thumb cache —
           and for audio the thumb route draws the *real* envelope, which beats the hashed
           silhouette below. So the server's answer wins whenever there is one. -->
      <img
        {src}
        class:cover={coverFit}
        alt=""
        loading="lazy"
        onerror={() => (thumbFailed = true)}
      />
    {:else if isSound}
      <!-- UI-SPEC §2.5: the sound card's stand-in for artwork, for the audio the backend cannot
           rasterise — in-pack payloads over the pull cap, which is most of a real Unity library.
           Arcane when identified, slate when not: the same two-colour rule as the rest of the card. -->
      <span class="wave">
        {#each bars as h, i (i)}
          <i class="bar" class:live={isPlaying} style="--h: {h}%; --i: {i}"></i>
        {/each}
      </span>
    {:else}
      <Icon name={typeIcon(item.media_type)} size={28} />
    {/if}

    {#if isSound && !unreachable}
      <!-- §2.5 puts ▶/◼ **in place of the thumbnail** — so it rides over the artwork rather than
           replacing one branch of it, and a sound card has a transport whether the thumb route drew
           a real envelope or the hashed bars stood in. It used to be a `▶` glyph inside an
           `aria-hidden` wrapper with no handler: clicking it selected the card, and a screen reader
           could not reach it at all. -->
      <span
        class="transport"
        class:on={isCurrent}
        role="button"
        tabindex="0"
        aria-pressed={isPlaying}
        title={isPlaying ? "Stop" : "Play"}
        aria-label={isPlaying ? `Stop ${item.name}` : `Play ${item.name}`}
        data-testid="card-play"
        onclick={(e) => {
          e.stopPropagation();
          toggleAudio();
        }}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            e.stopPropagation();
            toggleAudio();
          }
        }}
      >{isPlaying ? "◼" : "▶"}</span>
      {#if isCurrent}
        <!-- Only the active card carries a scrubber: 100 idle timelines is noise, and §2.5 asks
             for it "while that card is the active audio". -->
        <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
        <span class="scrub" data-testid="card-scrub" onclick={(e) => { e.stopPropagation(); scrubFrom(e); }}>
          <span class="scrub-fill" style="width: {Math.round(progress * 100)}%"></span>
        </span>
        <span class="clock num" data-testid="card-clock">
          {fmtClock(player.position)} / {fmtClock(clipSeconds)}
        </span>
      {/if}
      {#if player.error && isCurrent}
        <span class="audio-err" data-testid="card-audio-error">{player.error}</span>
      {/if}
    {/if}

    {#if unidentified}
      <!-- UI-SPEC §2.5: art at 30% + grayscale (handled on .thumb.unid) behind a large slate `?`.
           An unidentified asset should read as unidentified from across the grid, not by squinting
           at a 14px gem. -->
      <span class="qmark num" aria-hidden="true">?</span>
    {/if}

    <span class="corner">
      <!-- ✦ Identify now is a B11b addition, not part of UI-SPEC §2.5's card anatomy. It rides in
           the corner group rather than the bottom-right tray:
           three buttons down there reach past the *centre* of a 150px card, and a hover-revealed
           control sitting under the point people double-click swallows the double-click. It also
           belongs here by meaning — it acts on the identity the gem beside it reports. -->
      <span
        class="act id-btn"
        class:busy={identifying}
        role="button"
        tabindex="0"
        title={identifying ? "Identifying…" : "Identify now — embed and tag this asset"}
        aria-label={identifying ? `Identifying ${item.name}` : `Identify ${item.name}`}
        data-testid="card-identify"
        onclick={(e) => {
          e.stopPropagation();
          runIdentify();
        }}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            e.stopPropagation();
            runIdentify();
          }
        }}
      >✦</span>
      <!-- The star is always present, so the grid can *make* a favourite instead of only
           reporting one (UI-SPEC §2.5). A span with role=button: the card root is a <button>
           and nesting a real one inside it is invalid HTML. -->
      <span
        class="star"
        class:on={item.favorite}
        role="button"
        tabindex="0"
        aria-pressed={item.favorite}
        title={item.favorite ? "Remove from Favorites" : "Add to Favorites"}
        aria-label={item.favorite ? `Unfavourite ${item.name}` : `Favourite ${item.name}`}
        data-testid="card-star"
        onclick={(e) => {
          e.stopPropagation();
          onfavorite?.(item.asset_id, item.favorite);
        }}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            e.stopPropagation();
            onfavorite?.(item.asset_id, item.favorite);
          }
        }}
      >{item.favorite ? "★" : "☆"}</span>
      <span
        class="state"
        class:unknown={unidentified}
        title={unidentified ? "Unidentified" : item.status}
      >
        <Icon name={gem} size={14} />
      </span>
    </span>

    <!-- Bottom-left of the artwork: the size tier, and — when it applies — the imported badge.
         Never imported draws nothing at all: absence is the common case and must cost nothing. -->
    <span class="corner-bl">
      {#if item.size_tier}
        <span class="tier num t{item.size_tier}">{item.size_tier} · {fmtSize(item.size_bytes)}</span>
      {/if}
      <!-- Four states, one badge (DESIGN-GUIDELINES §7). Never imported draws nothing at all. -->
      {#if item.in_project && item.import_stale}
        <span
          class="imported stale"
          data-testid="card-imported"
          title="In the active project, but the source file has changed since"
        >✓ older copy</span>
      {:else if item.in_project}
        <span class="imported" data-testid="card-imported" title="Already in the active project">
          ✓ in project
        </span>
      {:else if item.in_other_projects > 0}
        <span
          class="imported elsewhere"
          data-testid="card-imported"
          title="Imported into {item.in_other_projects} other project{item.in_other_projects === 1
            ? ''
            : 's'}, not this one"
        >✓ in {item.in_other_projects} other{item.in_other_projects === 1 ? "" : "s"}</span>
      {/if}
    </span>

    <!-- Quick actions (UI-SPEC §2.5): bottom-right, hover-revealed, so they never collide with the
         corner's gem or the bottom-left tier badge. ⤢ full-size, ✦ identify, ⟡ stage. -->
    <span class="actions">
      <span
        class="act"
        role="button"
        tabindex="0"
        title="Full size — Quick Look"
        aria-label={`Quick Look ${item.name}`}
        data-testid="card-fullsize"
        onclick={(e) => {
          e.stopPropagation();
          onopen?.(item.asset_id);
        }}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            e.stopPropagation();
            onopen?.(item.asset_id);
          }
        }}
      >⤢</span>
      <!-- Inert while the source is away, and still focusable and still explaining itself: a
           control that vanishes teaches nothing, and one that silently does nothing is worse. -->
      <span
        class="act stage-btn"
        class:on={staged}
        class:inert={unreachable}
        role="button"
        tabindex="0"
        aria-disabled={unreachable}
        title={unreachable
          ? `${sources.get(item.source_id)?.name ?? "This library"} is unavailable — an import would skip this file`
          : staged
            ? "Staged — click to remove from the tray"
            : "Stage for import"}
        aria-label={staged ? `Unstage ${item.name}` : `Stage ${item.name}`}
        aria-pressed={staged}
        data-testid="card-stage"
        onclick={(e) => {
          e.stopPropagation();
          if (!unreachable) onStageClick();
        }}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            e.stopPropagation();
            if (!unreachable) onStageClick();
          }
        }}
      >⟡</span>
    </span>
  </div>

  <div class="meta">
    <span class="name"><i class="bullet" aria-hidden="true"></i>{item.name}</span>
    <span class="sub num">
      {#if item.package}
        <!-- A role=button span, not a <button>: the card root is already a <button> and nesting
             one inside it is invalid HTML — the same rule the star and ✦ follow. -->
        <span
          class="pack"
          role="button"
          tabindex="-1"
          title="Pack stats for {item.package}"
          data-testid="card-pack"
          onclick={(e) => {
            e.stopPropagation();
            onpack?.(item.package!);
          }}
          onkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              e.stopPropagation();
              onpack?.(item.package!);
            }
          }}
        >{item.package}</span>{#if stats}<span class="sep"> · </span>{/if}
      {/if}{stats}
    </span>
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
          role="button"
          tabindex="-1"
          title="Show everything tagged “{t}”"
          data-testid="card-chip"
          style={traitColor.has(t) ? `--chip: var(${traitColor.get(t)})` : ""}
          onclick={(e) => {
            e.stopPropagation();
            ontrait?.(t);
          }}
          onkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              e.stopPropagation();
              ontrait?.(t);
            }
          }}
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
    /* F1: a 1px status rail on the left edge — gold when confirmed, verdant when identified,
       grey when discovered/indexed (DESIGN-GUIDELINES §6). Kept as a width on the base so the
       rail never pops the layout. UI-SPEC §2.5 puts the status on the *left border*, not the
       whole perimeter — the 2026-08-13 mockup shows a full ring and loses to the spec on that
       point. The **width** is a known, recorded divergence: §2.5 says 3px, DESIGN-GUIDELINES §6
       says 1px and the app follows DG (ALIGNMENT-AUDIT F1, narrowed 2026-08-07). UI-SPEC's number
       is the stale one and still needs the same-change fix CLAUDE.md asks for. */
    border-left-width: 1px;
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
  /* UI-SPEC §2.5: "Confirmed cards: faint 1px gold ring at rest." A human vouched for this one;
     it reads as settled without having to hover or open the inspector. */
  .card.confirmed {
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--ls-gold) 35%, transparent);
  }
  .card.selected {
    border-color: var(--ls-gold);
    box-shadow: 0 0 0 1px var(--ls-gold);
  }
  /* F1: the left rail carries identification status, scannable across a whole row at a glance
     (the corner gem still carries the same fact on the artwork — the gem stays). Sits after
     .card:hover/.card.selected so the rail keeps its status colour while the other sides tint
     on hover; equal specificity means later wins here. */
  .card.identified {
    border-left-color: var(--ls-status-identified);
  }
  .card.confirmed {
    border-left-color: var(--ls-gold);
  }
  .card.unidentified {
    border-left-color: var(--ls-status-unidentified);
  }

  .thumb {
    position: relative;
    /* UI-SPEC §2.5: a 104px thumbnail region, not a square. A square thumb costs ~46px of card
       height per row at compact density and buys nothing — almost every asset here is wider than
       it is tall, and the taller frame just letterboxes harder. */
    height: 104px;
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
    /* F5 hover-spin: a slow, gentle lift-and-turn so previews feel alive on hover. `.thumb`'s
       overflow:hidden keeps the rotated preview inside the rounded artwork box, and the stage
       button / overlays are absolutely positioned above the image, so they stay put. */
    transition: transform 0.8s ease;
  }
  /* Tileable art fills the frame (UI-SPEC §2.5 "dark recess" reads as a hole when a 2048² albedo
     is letterboxed into a 104px strip). Subjects keep `contain` — see `coverFit`. */
  .thumb img.cover {
    object-fit: cover;
  }
  .card:hover .thumb img {
    transform: rotate(8deg) scale(1.05);
  }
  /* UI-SPEC §2.5: unidentified art sits at 30% + grayscale behind the slate `?`. */
  .thumb.unid img,
  .thumb.unid :global(.icon) {
    opacity: 0.3;
    filter: grayscale(1);
  }
  .qmark {
    position: absolute;
    font-size: 26px;
    line-height: 1;
    color: var(--ls-status-unidentified);
    pointer-events: none;
  }

  /* Sound card (UI-SPEC §2.5): ▶ + 5-bar mini-waveform. */
  .wave {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 46px;
    color: var(--ls-arcane);
  }
  .thumb.unid .wave {
    color: var(--ls-status-unidentified);
  }
  .transport {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    font-size: 10px;
    line-height: 1;
    color: var(--ls-arcane);
    background: color-mix(in srgb, var(--ls-bg) 78%, transparent);
    border: 1px solid currentColor;
    border-radius: 50%;
    cursor: pointer;
    z-index: 2;
  }
  .transport:hover,
  .transport.on {
    color: var(--ls-arcane-bright);
    background: color-mix(in srgb, var(--ls-bg) 92%, transparent);
  }
  .thumb.unid .transport {
    color: var(--ls-status-unidentified);
  }
  /* The scrubber sits on the bottom edge of the artwork, where a progress bar belongs and where it
     cannot collide with the tier badge or the hover actions. */
  .scrub {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 4px;
    background: color-mix(in srgb, var(--ls-text-dim) 30%, transparent);
    cursor: pointer;
    z-index: 2;
  }
  .scrub-fill {
    display: block;
    height: 100%;
    background: var(--ls-arcane);
  }
  .clock {
    position: absolute;
    left: var(--sp-2);
    top: var(--sp-2);
    font-size: 9px;
    line-height: 1;
    padding: 2px 4px;
    color: var(--ls-arcane);
    background: color-mix(in srgb, var(--ls-bg) 80%, transparent);
    border-radius: var(--r-sm);
    z-index: 2;
  }
  .audio-err {
    position: absolute;
    left: var(--sp-2);
    right: var(--sp-2);
    bottom: var(--sp-3);
    font-size: 9px;
    line-height: 1.25;
    color: var(--ls-danger);
    background: color-mix(in srgb, var(--ls-bg) 88%, transparent);
    border-radius: var(--r-sm);
    padding: 2px 4px;
    z-index: 2;
  }
  /* Animated EQ while playing (DESIGN-GUIDELINES §8) — gated on reduced motion below, where a
     playing card keeps the lit colour and drops the movement. */
  .wave .bar.live {
    animation: eq 900ms ease-in-out calc(var(--i) * 90ms) infinite alternate;
  }
  @keyframes eq {
    from {
      transform: scaleY(0.45);
    }
    to {
      transform: scaleY(1);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .wave .bar.live {
      animation: none;
    }
  }
  /* Optical centring: a ▶ glyph's bounding box is left-heavy, so a mathematically centred
     triangle reads as sitting too far left. Lives on `.transport` now — the decorative `.play`
     span it used to style is gone, replaced by a control that actually plays. */
  .transport {
    padding-left: 2px;
  }
  .wave .bar {
    width: 3px;
    height: var(--h);
    background: currentColor;
    border-radius: var(--r-pill);
    opacity: 0.75;
  }

  /* Star and status share the top-right corner (FEATURES §1.1) so the artwork's top-left stays
     clear for the multi-select box (UI-SPEC §2.5: hover-revealed, owned by LibraryScreen). */
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
  .corner-bl {
    position: absolute;
    left: var(--sp-1);
    bottom: var(--sp-1);
    display: flex;
    align-items: center;
    gap: 3px;
    max-width: calc(100% - 56px);
  }
  /* Check *and* colour *and* the word — never colour alone (DESIGN-GUIDELINES §9). */
  /* Amber is reserved for the one state you can act on; other projects are history and stay dim. */
  .imported.stale {
    color: var(--ls-warning);
    border-color: color-mix(in srgb, var(--ls-warning) 45%, transparent);
  }
  .imported.elsewhere {
    color: var(--ls-text-dim);
    border-color: var(--ls-border-strong);
  }
  .imported {
    padding: 1px 6px;
    font-size: var(--fs-xs);
    line-height: 1.4;
    color: var(--ls-success);
    background: color-mix(in srgb, var(--ls-bg) 82%, transparent);
    border: 1px solid color-mix(in srgb, var(--ls-success) 45%, transparent);
    border-radius: var(--r-sm);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tier {
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

  /* Hover-revealed quick actions (UI-SPEC §2.5): **two** buttons, bottom-right on the artwork,
     opposite the tier badge. Hovering a button keeps `.card:hover` true, so the tray does not
     flicker out from under the cursor.
     Two, not three: at compact density (150px card, 104px thumb) the card's own centre point
     falls inside this strip, and a third button pushed it left of centre — where an
     `opacity: 0` element still takes the click, so double-clicking the middle of a card hit a
     button nobody could see instead of opening Quick Look. `pointer-events` follows visibility
     for the same reason. */
  .actions {
    position: absolute;
    right: var(--sp-1);
    bottom: var(--sp-1);
    display: flex;
    align-items: center;
    gap: 3px;
    opacity: 0;
    pointer-events: none;
    transition: opacity 120ms ease;
  }
  .card:hover .actions,
  .actions:focus-within {
    opacity: 1;
    pointer-events: auto;
  }
  /* Same rule for the corner's ✦: invisible controls must not be clickable. */
  .id-btn {
    opacity: 0;
    pointer-events: none;
    transition: opacity 120ms ease;
  }
  .card:hover .id-btn,
  .id-btn:focus-visible {
    opacity: 1;
    pointer-events: auto;
  }
  .act {
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
  }
  .act:hover {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }
  /* Gold, like every other pack-name link in the app (Inspector, Packages). Underlined on hover
     rather than always, so a grid of 100 cards is not a wall of rules. */
  .sub .pack {
    color: var(--ls-gold);
    cursor: pointer;
  }
  .sub .pack:hover {
    color: var(--ls-gold-bright);
    text-decoration: underline;
  }
  .sub .sep {
    color: var(--ls-text-dim);
  }
  .tags .tag[role="button"] {
    cursor: pointer;
  }
  /* Inert, not hidden (UI-SPEC §11.1): the control still takes focus and still says why. Marked
     by shape as well as colour — the strike-through survives a monochrome display and a reader
     who cannot separate dim gold from gold (DESIGN-GUIDELINES §9). */
  .act.inert {
    color: var(--ls-text-dim);
    cursor: default;
    text-decoration: line-through;
  }
  .act.inert:hover {
    border-color: var(--ls-border);
    color: var(--ls-text-dim);
  }

  /* The `file unavailable` plate (UI-SPEC §11.1). Hatched rather than flat: an empty dark square
     is what a *failed* thumbnail looks like, and the whole point is that this is not a failure. */
  .thumb.unreachable {
    background:
      repeating-linear-gradient(
        45deg,
        transparent 0 6px,
        color-mix(in srgb, var(--ls-text-dim) 9%, transparent) 6px 12px
      ),
      var(--ls-bg);
  }
  .gone-plate {
    display: grid;
    place-items: center;
    gap: 2px;
    color: var(--ls-text-dim);
  }
  .gone-mark {
    font-size: 20px;
    line-height: 1;
    color: var(--ls-danger);
  }
  .gone-text {
    font-size: var(--fs-xs);
    letter-spacing: 0.04em;
  }
  /* A staged card keeps its ⟡ lit with the tray open, so the grid shows what is in the basket
     without hovering every card. */
  .stage-btn.on {
    border-color: var(--ls-gold);
    background: var(--ls-overlay);
  }
  .card:has(.stage-btn.on) .actions {
    opacity: 1;
    pointer-events: auto;
  }
  .act.busy {
    opacity: 1;
    pointer-events: auto;
    animation: idspin 900ms linear infinite;
  }
  @keyframes idspin {
    to {
      transform: rotate(360deg);
    }
  }

  .star {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    font-size: var(--fs-sm);
    line-height: 1;
    color: var(--ls-text-dim);
    text-shadow: 0 1px 2px var(--ls-bg);
    cursor: pointer;
  }
  .star:hover {
    color: var(--ls-gold);
  }
  .star.on {
    color: var(--ls-gold);
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
  /* UI-SPEC §2.5: "Unidentified: slate italic". A grid where an unappraised asset looks exactly
     like an appraised one hides the single fact this app exists to change. */
  .card.unidentified .name {
    font-style: italic;
    color: var(--ls-text-dim);
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

  /* Two rows of chips, at a fixed height: three trait names rarely fit on one line at compact
     density, and letting the row grow makes card height depend on how well-tagged an asset is —
     which turns the grid ragged and reflows every row as tags land. */
  .tags {
    display: flex;
    flex-wrap: wrap;
    align-content: flex-start;
    align-items: center;
    gap: var(--sp-1);
    height: 40px;
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
