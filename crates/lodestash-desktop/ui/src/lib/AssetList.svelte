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
    statusLabel,
    stage,
  } from "./api";
  import { sources } from "./sources.svelte";
  import { fmtClock } from "./audio.svelte";

  /**
   * Same tray the cards reach, through the same context the shell owns (App.svelte
   * `setContext("ls:staging", …)`). A row is another way into the one shortlist, not a second one.
   * Rendered outside the shell there is no context, and staging falls back to the direct command
   * exactly as `Card.svelte` does.
   */
  const staging = getContext<{
    isStaged?: (id: number) => boolean;
    toggleStage?: (id: number) => void;
  }>("ls:staging");

  let {
    items,
    selected,
    onselect,
    onopen,
    onmenu,
    onfavorite,
    onidentify,
    onpack,
    trashedOnly = false,
    onrestore,
  }: {
    items: GridItem[];
    selected: number | null;
    onselect: (id: number) => void;
    onopen: (id: number) => void;
    onmenu?: (e: MouseEvent, id: number) => void;
    /**
     * D6 / UI-SPEC §2.5: rows carry the same star, Quick Look and stage the cards do. Switching to
     * list used to cost you all three (ALIGNMENT-AUDIT F6). Reports the *current* flag; the shell
     * inverts it, exactly as the card and the context menu do.
     */
    onfavorite?: (id: number, favorite: boolean) => void;
    /** Unidentified rows lead with ✦ — on a row you cannot see the art, so appraising is the
     *  fastest useful thing. */
    onidentify?: (id: number) => void;
    /** Pack name → Pack stats (UI-SPEC §15). Reported up: one modal per screen, not per row. */
    onpack?: (name: string) => void;
    /** Trash view: a per-row Restore (UI-SPEC §10 — the spec'd trash table). */
    trashedOnly?: boolean;
    onrestore?: (id: number) => void;
  } = $props();

  const isUnidentified = (s: string) => s === "discovered" || s === "indexed";

  const isStaged = (id: number) => staging?.isStaged?.(id) ?? false;
  function toggleStage(id: number) {
    if (staging?.toggleStage) staging.toggleStage(id);
    else void stage([id]);
  }

  /** A row that cannot be thumbnailed just shows nothing rather than a broken-image glyph. */
  const hideBroken = (e: Event) => ((e.currentTarget as HTMLImageElement).style.display = "none");
</script>

<div class="list" role="table" aria-label="Assets">
  <!-- Type left the column set: it is already the diamond in the name cell, and the column it
       freed is the pack link §2.5 has always asked rows for. -->
  <div class="head" role="row">
    <span role="columnheader">Name</span>
    <span role="columnheader">Pack</span>
    <span role="columnheader" class="right">Size</span>
    <span role="columnheader" class="right">Tris</span>
    <span role="columnheader">Status</span>
    <span role="columnheader"></span>
  </div>
  {#each items as item (item.asset_id)}
    <button
      class="row"
      class:on={selected === item.asset_id}
      style="--tint: var({typeToken(item.media_type)})"
      role="row"
      title={item.rel_path}
      onclick={() => onselect(item.asset_id)}
      ondblclick={() => onopen(item.asset_id)}
      oncontextmenu={(e) => {
        e.preventDefault();
        onmenu?.(e, item.asset_id);
      }}
    >
      <span class="cell name" role="cell">
        <span
          class="thumb"
          class:unid={isUnidentified(item.status)}
          class:unreachable={sources.unavailable(item.source_id)}
        >
          {#if sources.unavailable(item.source_id)}
            <!-- Row parity with the card plate (UI-SPEC §11.1 / §2.5). At 22px there is no room
                 for the words, so the row carries the mark and the title carries the sentence. -->
            <span
              class="gone"
              data-testid="row-unavailable"
              title="{sources.get(item.source_id)?.name ?? 'This library'} is unavailable — the file cannot be reached"
            >✕</span>
          {:else if isUnidentified(item.status)}
            <span class="q">?</span>
          {:else if item.has_preview}
            <img src={previewUrl(item.asset_id)} alt="" loading="lazy" />
          {:else if item.media_type !== "model_3d" && !item.in_pack}
            <img src={thumbUrl(item.asset_id)} alt="" loading="lazy" onerror={hideBroken} />
          {:else}
            <Icon name={typeIcon(item.media_type)} size={16} />
          {/if}
        </span>
        <span class="bullet" aria-hidden="true"></span>
        <!-- The row root is a <button>, so a real control inside it would be invalid HTML — same
             rule as the grid's multi-select box. A role=button span keeps the click. -->
        <span
          class="star"
          class:on={item.favorite}
          role="button"
          tabindex="0"
          aria-pressed={item.favorite}
          title={item.favorite ? "Remove from Favorites" : "Add to Favorites"}
          aria-label={item.favorite ? `Unfavourite ${item.name}` : `Favourite ${item.name}`}
          data-testid="row-star"
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
        <span class="label" class:unid={isUnidentified(item.status)}>{item.name}</span>
        {#if item.in_project}
          <span
            class="imported"
            class:stale={item.import_stale}
            data-testid="row-imported"
            title={item.import_stale
              ? "In the active project, but the source file has changed since"
              : "Already in the active project"}
          >✓</span>
        {/if}
      </span>
      <span class="cell pack dim" role="cell">
        {#if item.package}
          <!-- §2.5 spells the row out as "name · pack (link) · size badge …". D6 built every other
               column and left this one a plain span. -->
          <span
            class="packlink"
            role="button"
            tabindex="-1"
            title="Pack stats for {item.package}"
            data-testid="row-pack"
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
          >{item.package}</span>
        {:else}
          —
        {/if}
      </span>
      <span class="cell right" role="cell">
        {#if item.size_tier}
          <span class="tier num t{item.size_tier}" title={fmtSize(item.size_bytes)}>
            {item.size_tier}
          </span>
        {/if}
      </span>
      <!-- The stats column carries whatever the type actually has: triangles for geometry, clip
           length for a sound. A column of — down 1,841 audio rows says nothing (UI-SPEC §2.5). -->
      <span class="cell right num" role="cell">
        {#if item.media_type === "audio" && item.duration_ms}
          {fmtClock(item.duration_ms / 1000)}
        {:else}
          {item.triangles ? item.triangles.toLocaleString() : "—"}
        {/if}
      </span>
      <span class="cell status" role="cell" class:dim={item.status !== "confirmed"}>
        <span class="gem" aria-hidden="true"></span>
        {statusLabel(item.status)}
      </span>
      {#if !trashedOnly}
        <!-- Actions live in their own trailing column, not floating over content: a row has no
             artwork to hide them on, and a hover-only control in a 32px row is a smaller target
             than the row itself. -->
        <span class="cell acts" role="cell">
          {#if isUnidentified(item.status)}
            <span
              class="act ai"
              role="button"
              tabindex="0"
              title="Identify now"
              aria-label={`Identify ${item.name}`}
              data-testid="row-identify"
              onclick={(e) => {
                e.stopPropagation();
                onidentify?.(item.asset_id);
              }}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  e.stopPropagation();
                  onidentify?.(item.asset_id);
                }
              }}
            >✦</span>
          {/if}
          <span
            class="act"
            role="button"
            tabindex="0"
            title="Quick Look"
            aria-label={`Quick Look ${item.name}`}
            data-testid="row-open"
            onclick={(e) => {
              e.stopPropagation();
              onopen(item.asset_id);
            }}
            onkeydown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                e.stopPropagation();
                onopen(item.asset_id);
              }
            }}
          >⤢</span>
          {#if !isUnidentified(item.status)}
            <span
              class="act"
              class:on={isStaged(item.asset_id)}
              class:inert={sources.unavailable(item.source_id)}
              role="button"
              tabindex="0"
              aria-pressed={isStaged(item.asset_id)}
              aria-disabled={sources.unavailable(item.source_id)}
              title={sources.unavailable(item.source_id)
                ? `${sources.get(item.source_id)?.name ?? "This library"} is unavailable — an import would skip this file`
                : isStaged(item.asset_id)
                  ? "Staged — click to remove"
                  : "Stage for import"}
              aria-label={isStaged(item.asset_id) ? `Unstage ${item.name}` : `Stage ${item.name}`}
              data-testid="row-stage"
              onclick={(e) => {
                e.stopPropagation();
                if (!sources.unavailable(item.source_id)) toggleStage(item.asset_id);
              }}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  e.stopPropagation();
                  if (!sources.unavailable(item.source_id)) toggleStage(item.asset_id);
                }
              }}
            >⟡</span>
          {/if}
        </span>
      {/if}
      {#if trashedOnly}
        <span class="cell right" role="cell">
          <!-- The row root is a <button>, so a real control inside it would be invalid HTML —
               same rule as the grid's multi-select box. A role=button span keeps the click. -->
          <span
            class="restore"
            role="button"
            tabindex="0"
            data-testid="row-restore"
            onclick={(e) => {
              e.stopPropagation();
              onrestore?.(item.asset_id);
            }}
            onkeydown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.stopPropagation();
                onrestore?.(item.asset_id);
              }
            }}
          >
            Restore
          </span>
        </span>
      {/if}
    </button>
  {/each}
</div>

<style>
  .list {
    display: flex;
    flex-direction: column;
  }
  /* One grid template shared by the header and every row, so columns stay aligned without a
     real <table> — which would fight the virtualisation this grows into. */
  .head,
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 150px 62px 74px 112px 58px;
    gap: var(--sp-3);
    align-items: center;
    padding: 0 var(--sp-2);
    text-align: left;
  }
  .head {
    height: 26px;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    border-bottom: 1px solid var(--ls-border);
    position: sticky;
    top: 0;
    background: var(--ls-bg);
    z-index: 1;
  }
  .row {
    height: var(--row-h);
    border-bottom: 1px solid color-mix(in srgb, var(--ls-border) 50%, transparent);
    color: var(--ls-text-muted);
    font-size: var(--fs-sm);
  }
  .row:hover {
    background: var(--ls-surface);
    color: var(--ls-text);
  }
  .row.on {
    background: var(--ls-surface-raised);
    box-shadow: inset 2px 0 0 var(--ls-gold);
    color: var(--ls-text);
  }

  .cell {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-width: 0;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .thumb {
    flex: none;
    /* Tracks the density-driven row height (DESIGN-GUIDELINES §4: --row-h 32/36/40px) with
       a fixed 6px breathing margin — a fixed 22px thumb sat oddly in a 40px cozy row and
       shrunk relative to the grid cards when density changed. */
    width: calc(var(--row-h) - 12px);
    height: calc(var(--row-h) - 12px);
    display: grid;
    place-items: center;
    background: var(--ls-bg);
    border-radius: var(--r-sm);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--tint) 25%, transparent);
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .star {
    flex: none;
    display: grid;
    place-items: center;
    width: 14px;
    height: 14px;
    color: var(--ls-text-dim);
    font-size: var(--fs-xs);
    cursor: pointer;
  }
  .star.on,
  .star:hover {
    color: var(--ls-gold);
  }

  /* The type mark rows never had — the same 7px rotated square the card uses, so a row and a card
     say "what kind" with the same shape. */
  .bullet {
    flex: none;
    width: 7px;
    height: 7px;
    background: var(--tint);
    transform: rotate(45deg);
  }

  .label.unid {
    font-style: italic;
    color: var(--ls-text-dim);
  }
  .thumb.unid {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--ls-status-unidentified) 30%, transparent);
  }
  .thumb .q {
    color: var(--ls-status-unidentified);
    font-size: var(--fs-sm);
    line-height: 1;
  }
  /* Row parity with the card's plate (UI-SPEC §11.1). */
  .thumb.unreachable {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--ls-danger) 30%, transparent);
  }
  .thumb .gone {
    color: var(--ls-danger);
    font-size: var(--fs-sm);
    line-height: 1;
  }
  .act.inert {
    color: var(--ls-text-dim);
    cursor: default;
    text-decoration: line-through;
  }

  .pack {
    color: var(--ls-gold);
  }
  .packlink {
    cursor: pointer;
  }
  .packlink:hover {
    color: var(--ls-gold-bright);
    text-decoration: underline;
  }
  /* A row has no room for the project's name — the check, with the full text on hover. */
  .imported {
    flex: none;
    color: var(--ls-success);
    font-size: var(--fs-xs);
  }
  .imported.stale {
    color: var(--ls-warning);
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-xs);
  }
  .gem {
    flex: none;
    width: 6px;
    height: 6px;
    background: currentColor;
    transform: rotate(45deg);
  }

  .tier {
    display: inline-block;
    padding: 0 5px;
    font-size: var(--fs-xs);
    line-height: 1.5;
    color: var(--tier-fg, var(--ls-text-muted));
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

  /* Dim at rest, full contrast on the hovered or selected row: present without shouting, and a
     real target either way — never opacity:0, which takes clicks it cannot show. */
  .acts {
    display: flex;
    gap: 3px;
    justify-content: flex-end;
    opacity: 0.45;
  }
  .row:hover .acts,
  .row.on .acts,
  .acts:focus-within {
    opacity: 1;
  }
  .act {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    font-size: 11px;
    line-height: 1;
    color: var(--ls-gold);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  .act:hover {
    border-color: var(--ls-gold);
    color: var(--ls-gold-bright);
  }
  .act.on {
    border-color: var(--ls-gold);
    background: var(--ls-overlay);
  }
  .act.ai {
    color: var(--ls-arcane);
    border-color: color-mix(in srgb, var(--ls-arcane) 45%, transparent);
  }
  .star:focus-visible,
  .act:focus-visible {
    box-shadow: var(--el-focus-ring);
    outline: none;
  }
  .type {
    color: var(--tint);
    font-size: var(--fs-xs);
  }
  .dim {
    color: var(--ls-text-dim);
  }
  .right {
    text-align: right;
  }
  .restore {
    border: 1px solid var(--ls-border);
    border-radius: 5px;
    background: transparent;
    color: var(--ls-text-dim);
    font: inherit;
    font-size: var(--fs-xs);
    padding: 2px 8px;
    cursor: default;
  }
  .restore:hover {
    border-color: var(--ls-border-strong);
    color: var(--ls-text);
    background: var(--ls-hover);
  }
</style>
