<script lang="ts">
  import { getContext } from "svelte";
  import Icon from "./Icon.svelte";
  import CompareRows from "./CompareRows.svelte";
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
    collectionMembers,
    collectionAddAsset,
    collectionRemoveAsset,
    type AssetDetail,
    type CompareRow,
    type RelatedAsset,
    type CollectionRow,
  } from "./api";
  import { view } from "./nav.svelte";
  import { traitColor } from "./traits.svelte";
  import { sources } from "./sources.svelte";
  import { player, fmtClock } from "./audio.svelte";

  /**
   * The staging tray, through the same context the cards use (App.svelte `setContext("ls:staging")`).
   *
   * UI-SPEC §13.10 puts **⟡ Stage / ✓ Staged** in this action row, beside Import. It was the one
   * surface without it until 2026-08-26: the card had ⟡, the list row had ⟡, and the panel you
   * open to *decide* about a single asset had no way to add it to the shortlist — you had to go
   * back to the grid and find the card again. §16.2 already assumes the pairing, offering Export
   * to folder here "because a single pick should not have to be staged first".
   */
  const staging = getContext<{
    isStaged?: (id: number) => boolean;
    toggleStage?: (id: number) => void;
  }>("ls:staging");

  let {
    assetId,
    favorite = false,
    trashed = false,
    onidentify,
    onfavorite,
    ontrash,
    onrestore,
    onremove,
    ondelete,
    onimport,
    onexport,
  }: {
    assetId: number | null;
    /** Carried by the grid row, so the star reflects the same state the card draws. */
    favorite?: boolean;
    /** True when the selection is in the Trash — the actions change to Restore. */
    trashed?: boolean;
    /** Per-asset Identify now (B11b in the inspector): flag dirty + run one bounded appraise pass. */
    onidentify: () => void;
    onfavorite: (on: boolean) => void;
    ontrash: () => void;
    onrestore: () => void;
    onremove: () => void;
    /** Opens the confirm dialog. This panel never deletes anything directly. */
    ondelete: () => void;
    /** `⇢ Add to project` (FEATURES §7.2) — queue into the open Unity project. */
    onimport: () => void;
    /** **Export to folder** (UI-SPEC §16.2): copy this asset anywhere, no editor involved. */
    onexport: () => void;
  } = $props();

  let staged = $derived(assetId != null && (staging?.isStaged?.(assetId) ?? false));

  /**
   * UI-SPEC §13 has three states, not one:
   *
   * - **collapsed** — nothing selected (or the user closed it): a 26px edge tab. The rail never
   *   holds 300px open to print "Select an asset." (ALIGNMENT-AUDIT N5.3).
   * - **compare** — exactly two picked: 460px, two columns, differing rows gold. This is the pane
   *   Duplicates (§5) reviews groups in.
   * - **one asset** — the stat block, unchanged.
   */
  let compareIds = $derived(view.selectedIds.length === 2 ? [...view.selectedIds] : null);
  let compare = $state<AssetDetail[] | null>(null);
  $effect(() => {
    const ids = compareIds;
    if (!ids) {
      compare = null;
      return;
    }
    let live = true;
    Promise.all(ids.map((id) => assetDetail(id)))
      .then((d) => {
        // A row deleted between the pick and the read comes back null; two is the only shape the
        // pane can render, so anything else collapses back to the single-asset view.
        const both = d.filter((x): x is AssetDetail => !!x);
        if (live) compare = both.length === 2 ? both : null;
      })
      .catch(() => {
        if (live) compare = null;
      });
    return () => {
      live = false;
    };
  });

  /**
   * A manual collapse sticks until the next selection — selecting something is an unambiguous ask
   * to see it, and leaving the rail shut then would swallow the click silently.
   */
  $effect(() => {
    if (assetId != null) view.inspectorOpen = true;
  });

  let collapsed = $derived(!view.inspectorOpen || (assetId == null && !compareIds));

  /** Compare rows: the fields two assets can actually be judged on, per modality. */
  type CmpRow = CompareRow;
  let compareRows = $derived.by<CmpRow[]>(() => {
    const pair = compare;
    if (!pair || pair.length !== 2) return [];
    const [a, b] = pair;
    const row = (label: string, f: (d: AssetDetail) => string): CmpRow => ({
      label,
      values: [f(a), f(b)],
    });
    const rows: CmpRow[] = [
      row("Status", (d) => statusLabel(d.status)),
      row("Type", (d) => d.media_type),
      row("Size", (d) => fmtSize(d.size_bytes)),
      row("Source", (d) => d.source ?? "—"),
      row("Tags", (d) => String(d.tags.length)),
    ];
    if (a.triangles != null || b.triangles != null) {
      rows.push(row("Tris", (d) => (d.triangles != null ? d.triangles.toLocaleString() : "—")));
    }
    if (a.duration_ms != null || b.duration_ms != null) {
      rows.push(row("Length", (d) => (d.duration_ms != null ? fmtDuration(d.duration_ms) : "—")));
    }
    if (a.width_px != null || b.width_px != null) {
      rows.push(
        row("Pixels", (d) => (d.width_px != null ? `${d.width_px}×${d.height_px ?? "?"}` : "—")),
      );
    }
    return rows;
  });

  /** Drop the comparison down to one side — the pick set, not the catalog. Nothing is deleted. */
  function keepOnly(i: 0 | 1) {
    const pair = compare;
    if (pair) view.selectedIds = [pair[i].asset_id];
  }

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

  /**
   * Audio transport (UI-SPEC §13.2). The same one element the grid cards drive — there is exactly
   * one `<audio>` in the app, so the inspector and a card can never disagree about what is playing.
   *
   * The scrubber appears **after first play**, with a hint before it: a timeline you cannot yet
   * scrub is a control that lies, and §13.2 asks for the hint by name.
   */
  let isSound = $derived(detail?.media_type === "audio");
  let isCurrent = $derived(!!detail && player.current === detail.asset_id);
  let isPlaying = $derived(!!detail && player.isPlaying(detail.asset_id));
  let clipSeconds = $derived(
    isCurrent && player.duration ? player.duration : (detail?.duration_ms ?? 0) / 1000,
  );
  let progress = $derived(isCurrent && clipSeconds ? Math.min(1, player.position / clipSeconds) : 0);
  /** Bars for the static waveform (§13.1). Hashed off the id like the card's, so the same asset
   *  looks the same everywhere; the real envelope lives in the thumb route when it can render one. */
  let waveBars = $derived.by(() => {
    let h = Math.imul(detail?.asset_id ?? 1, 2654435761);
    return Array.from({ length: 24 }, () => {
      h = Math.imul(h ^ (h >>> 15), 2246822519);
      return 30 + (Math.abs(h) % 71);
    });
  });

  function scrubFrom(e: MouseEvent) {
    const el = e.currentTarget as HTMLElement;
    const r = el.getBoundingClientRect();
    player.seek((e.clientX - r.left) / Math.max(1, r.width));
  }

  // ── Tag-editing slice (G4–G9) ──────────────────────────────────────────────────
  // TRAITS (G4): the quick-add write path + the asset's real tag set (`detail.tags`,
  // read through AssetDetail) — AI labels show their confidence and a ✕ to reject.
  // `bulkTag` with a single id is the same writer the bulk bar (B9d) uses, so the op
  // is undoable.
  let sessionTags = $state<string[]>([]);
  let tagValue = $state("");
  let tagBusy = $state(false);
  let tagMsg = $state<string | null>(null);

  /** Reject a tag on the selected asset (UI-SPEC §9): removes it via the same undoable op. */
  async function rejectTag(namespace: string, value: string) {
    const id = detail?.asset_id;
    if (id == null || tagBusy) return;
    tagBusy = true;
    try {
      await bulkTag([id], namespace, value, false);
      sessionTags = sessionTags.filter((v) => v !== value);
      view.dataVersion += 1;
    } catch (e) {
      tagMsg = String(e);
    } finally {
      tagBusy = false;
    }
  }

  // LINKED FILES (G5) / SIMILAR (G7): both draw the same `related_assets` edges —
  // one fetch, two views of it. `null` while loading so the panels can say so.
  let related = $state<RelatedAsset[] | null>(null);

  // COLLECTIONS (G6): the rows come from `collections()`; each collection's members come
  // from `collectionMembers(c.id)` — one read per collection (collections are few). The
  // chip on-state is `memberIds[c.id].includes(selectedId)`, so it is the real catalog.
  let collectionsRows = $state<CollectionRow[]>([]);
  let memberIds = $state<Record<number, number[]>>({});

  // Per-asset state resets with the selection. Kept apart from the fetch effect so a
  // `dataVersion` bump (e.g. the collection toggle the user just made) re-asks the
  // data without wiping what is already on screen.
  $effect(() => {
    const id = assetId;
    related = null;
    collectionsRows = [];
    sessionTags = [];
    memberIds = {};
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
        return cs;
      })
      .then((cs) => {
        if (stale) return;
        // One `collection_members` read per collection (B10j) — the chip draws from the ids.
        for (const c of cs) {
          collectionMembers(c.id)
            .then((ids) => {
              if (!stale) memberIds = { ...memberIds, [c.id]: ids };
            })
            .catch(() => {});
        }
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

  /** Toggle one collection's membership via the writer op, then re-read that collection. */
  async function toggleCollection(c: CollectionRow) {
    const id = detail?.asset_id;
    if (id == null) return;
    const on = !(memberIds[c.id]?.includes(id) ?? false);
    try {
      if (on) await collectionAddAsset(c.id, id);
      else await collectionRemoveAsset(c.id, id);
      view.dataVersion += 1;
      // Refresh this collection so the chip shows what the backend actually wrote.
      memberIds = { ...memberIds, [c.id]: await collectionMembers(c.id) };
    } catch (e) {
      tagMsg = String(e);
    }
  }

  /** Chip on-state: the selected asset is one of `c`'s members (B10j). */
  function inCollection(c: CollectionRow): boolean {
    if (detail == null) return false;
    return memberIds[c.id]?.includes(detail.asset_id) ?? false;
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

{#if collapsed}
  <!-- The best empty state is the one that takes no room (DESIGN-GUIDELINES §7). -->
  <aside class="edge" data-testid="inspector-edge">
    {#if assetId != null}
      <button class="vert" onclick={() => (view.inspectorOpen = true)} aria-label="Open inspector">
        INSPECTOR ‹
      </button>
    {:else}
      <span class="vert dim">INSPECTOR</span>
    {/if}
  </aside>
{:else if compare}
  <aside class="inspector wide" data-testid="inspector-compare">
    <header class="cmp-head">
      <span class="cmp-kicker">Comparing 2</span>
      <button class="cmp-x" onclick={() => view.clearSelection()} aria-label="Clear comparison">✕</button>
    </header>
    <div class="cmp-cols">
      {#each compare as d (d.asset_id)}
        <div class="cmp-col">
          <div class="cmp-art">
            {#if d.has_preview}
              <img src={previewUrl(d.asset_id)} alt="" />
            {:else if d.media_type !== "model_3d" && !d.in_pack}
              <img src={thumbUrl(d.asset_id)} alt="" onerror={hideBroken} />
            {:else}
              <Icon name={typeIcon(d.media_type)} size={28} />
            {/if}
          </div>
          <div class="cmp-name" title={d.rel_path}>{d.name}</div>
        </div>
      {/each}
    </div>
    <div class="cmp-table">
      <CompareRows rows={compareRows} />
    </div>
    <footer class="cmp-foot">
      <button onclick={() => keepOnly(0)}>Keep left</button>
      <button onclick={() => keepOnly(1)}>Keep right</button>
    </footer>
  </aside>
{:else if detail}
  <aside class="inspector" style="--tint: {tint}">
    <div class="hero" data-testid="inspector-hero">
      {#if isSound}
        <!-- §13.1: sounds show a static waveform, and the bars animate while it plays. -->
        <span class="wave" class:live={isPlaying}>
          {#each waveBars as h, i (i)}
            <i class="bar" style="--h: {h}%; --i: {i}"></i>
          {/each}
        </span>
      {:else if detail.has_preview}
        <img src={previewUrl(detail.asset_id)} alt="" />
      {:else if detail.media_type !== "model_3d" && !detail.in_pack}
        <img src={thumbUrl(detail.asset_id)} alt="" onerror={hideBroken} />
      {:else}
        <Icon name={typeIcon(detail.media_type)} size={40} />
      {/if}
    </div>

    {#if isSound}
      <!-- UI-SPEC §13.2. Round ▶/◼, then the scrubber — but only once it has been played: a
           timeline you cannot scrub yet is a control that lies about what it does. -->
      <div class="transport" data-testid="inspector-transport">
        <button
          class="play"
          onclick={() => detail && player.toggle(detail.asset_id)}
          aria-pressed={isPlaying}
          aria-label={isPlaying ? `Stop ${detail.name}` : `Play ${detail.name}`}
          title={isPlaying ? "Stop" : "Play"}
          data-testid="inspector-play"
        >{isPlaying ? "◼" : "▶"}</button>
        {#if isCurrent}
          <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
          <span class="track" data-testid="inspector-scrub" onclick={scrubFrom}>
            <span class="fill" style="width: {Math.round(progress * 100)}%"></span>
          </span>
          <span class="clock num" data-testid="inspector-clock">
            {fmtClock(player.position)} / {fmtClock(clipSeconds)}
          </span>
        {:else}
          <span class="hint">play to scrub the timeline</span>
        {/if}
      </div>
      {#if player.error && isCurrent}
        <p class="msg danger" data-testid="inspector-audio-error">{player.error}</p>
      {/if}
    {/if}

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
      {#if (detail?.tags.length ?? 0) === 0 && sessionTags.length === 0}
        <p class="hint">No tags yet — add one below, or run Identify to propose labels.</p>
      {:else}
        <div class="tagchips" data-testid="inspector-tagchips">
          {#each detail?.tags ?? [] as t (t.namespace + ":" + t.value)}
            <span
              class="tag"
              style={traitColor.has(t.value) ? `--chip: var(${traitColor.get(t.value)})` : ""}
              title={t.namespace === "free" ? "hand-added" : `${t.namespace} · ${Math.round((t.confidence ?? 0) * 100)}%`}
            >
              {t.value}{#if t.confidence != null}<em class="conf" aria-label="confidence"
                  >{Math.round(t.confidence * 100)}%</em
                >{/if}
              <button
                class="x"
                onclick={() => rejectTag(t.namespace, t.value)}
                aria-label={`Remove tag ${t.value}`}
                data-testid="reject-tag"
              >✕</button>
            </span>
          {/each}
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
              class:on={inCollection(c)}
              onclick={() => toggleCollection(c)}
              title="{c.asset_count} assets"
              data-testid="collection-toggle-{c.id}"
            >
              {inCollection(c) ? "✓ " : "+ "}{c.name}
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
        onclick={onidentify}
        data-testid="action-identify"
        title="Flag this asset dirty and run one bounded appraise pass — local tags + embeddings (B11b)."
      >
        ✦ Identify
      </button>
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
        <!-- Inert while the drive is out (UI-SPEC §11.1 / §16.2): an import that cannot read the
             file would queue a job whose only outcome is a skip. Same rule as the card's ⟡. -->
        {@const gone = !!detail && sources.unavailable(detail.source_id)}
        <button
          onclick={() => !gone && onimport()}
          class:inert={gone}
          aria-disabled={gone}
          data-testid="action-import"
          title={gone
            ? `${sources.get(detail?.source_id ?? -1)?.name ?? "This library"} is unavailable — an import would skip this file`
            : "Queue this asset into the open Unity project (ENGINE-API §5.2)"}
        >
          ⇢ Add to project
        </button>
        <!-- ⟡ Stage sits **before** Export, matching §13.10's order: the tray is the cheap,
             reversible pick; exporting writes files. Inert on an unreachable file for the same
             reason ⟡ is on the card — staging what an import would skip is a promise the tray
             cannot keep (§11.1). -->
        <button
          class="stage"
          class:on={staged}
          class:inert={gone}
          aria-disabled={gone}
          aria-pressed={staged}
          onclick={() => !gone && assetId != null && staging?.toggleStage?.(assetId)}
          data-testid="action-stage"
          title={gone
            ? `${sources.get(detail?.source_id ?? -1)?.name ?? "This library"} is unavailable — an import would skip this file`
            : staged
              ? "In the staging tray — click to take it out"
              : "Add to the staging tray (UI-SPEC §16.2)"}
        >
          {staged ? "✓ Staged" : "⟡ Stage"}
        </button>
        <button
          onclick={onexport}
          data-testid="action-export"
          title="Copy this file into a folder you choose. The original is never moved."
        >
          ⤓ Export to folder…
        </button>
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
  </aside>
{/if}

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
  /* §13.1 waveform + §13.2 transport. */
  .hero .wave {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 64px;
    color: var(--ls-arcane);
  }
  .hero .wave .bar {
    width: 3px;
    height: var(--h);
    background: currentColor;
    border-radius: var(--r-pill);
    opacity: 0.75;
  }
  .hero .wave.live .bar {
    animation: eq 900ms ease-in-out calc(var(--i) * 40ms) infinite alternate;
  }
  @keyframes eq {
    from {
      transform: scaleY(0.4);
    }
    to {
      transform: scaleY(1);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .hero .wave.live .bar {
      animation: none;
    }
  }
  .transport {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin-top: var(--sp-2);
  }
  .transport .play {
    flex: none;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding-left: 2px;
    font-size: 11px;
    line-height: 1;
    color: var(--ls-arcane);
    background: none;
    border: 1px solid currentColor;
    border-radius: 50%;
    cursor: pointer;
  }
  .transport .play:hover {
    color: var(--ls-arcane-bright);
  }
  .transport .track {
    flex: 1;
    min-width: 0;
    height: 4px;
    background: color-mix(in srgb, var(--ls-text-dim) 30%, transparent);
    border-radius: var(--r-pill);
    cursor: pointer;
  }
  .transport .fill {
    display: block;
    height: 100%;
    background: var(--ls-arcane);
    border-radius: var(--r-pill);
  }
  .transport .clock {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .transport .hint {
    flex: 1;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }

  .actions button.inert {
    color: var(--ls-text-dim);
    cursor: default;
    text-decoration: line-through;
  }
  .actions button:hover {
    border-color: var(--ls-gold);
  }
  .actions .star.on,
  .actions .stage.on {
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
  /* UI-SPEC §13 compare: 460px, wide enough for two stat columns that still read as columns. */
  .inspector.wide {
    width: 460px;
  }

  /* Collapsed: 26px of edge, not 300px of instruction. */
  .edge {
    width: 26px;
    flex: none;
    background: var(--ls-surface);
    border-left: 1px solid var(--ls-border);
    display: grid;
    place-items: center;
  }
  .vert {
    writing-mode: vertical-rl;
    transform: rotate(180deg);
    font-size: var(--fs-xs);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ls-text-muted);
    background: none;
    border: 0;
    padding: var(--sp-2) 0;
    font-family: inherit;
    cursor: pointer;
  }
  .vert.dim {
    color: var(--ls-text-dim);
    cursor: default;
  }
  button.vert:hover {
    color: var(--ls-gold);
  }
  button.vert:focus-visible {
    box-shadow: var(--el-focus-ring);
    outline: none;
  }

  .cmp-head {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin-bottom: var(--sp-3);
  }
  .cmp-kicker {
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.09em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
  }
  .cmp-x {
    margin-left: auto;
    background: none;
    border: 0;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    cursor: pointer;
  }
  .cmp-x:hover {
    color: var(--ls-text);
  }
  .cmp-cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--sp-2);
  }
  .cmp-art {
    height: 110px;
    display: grid;
    place-items: center;
    background: var(--ls-bg);
    border-radius: var(--r-md);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--ls-border-strong) 60%, transparent);
    overflow: hidden;
  }
  .cmp-art img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .cmp-name {
    margin-top: var(--sp-1);
    font-size: var(--fs-sm);
    color: var(--ls-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cmp-table {
    margin-top: var(--sp-3);
    padding-top: var(--sp-3);
    border-top: 1px solid var(--ls-border);
  }
  .cmp-foot {
    display: flex;
    gap: var(--sp-2);
    margin-top: var(--sp-4);
  }
  .cmp-foot button {
    flex: 1;
    height: var(--control-h);
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    background: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    cursor: pointer;
  }
  .cmp-foot button:hover {
    border-color: var(--ls-gold);
    color: var(--ls-text);
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
  .tag .conf {
    font-style: normal;
    opacity: 0.65;
    margin-left: 2px;
  }
  .tag .x {
    margin-left: 4px;
    background: none;
    border: 0;
    padding: 0 1px;
    color: inherit;
    font-size: 9px;
    line-height: 1;
    cursor: pointer;
    opacity: 0.5;
  }
  .tag .x:hover {
    opacity: 1;
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
