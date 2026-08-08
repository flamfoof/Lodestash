<script lang="ts">
  /**
   * Stats → Overview (UI-SPEC §8.1): stat tiles, size by type, disk weight by pack, heaviest
   * assets, insights.
   *
   * Stats → Tag Graph (UI-SPEC §8.2): the tag↔tag co-occurrence graph of the identified library —
   * gold nodes on a deterministic ring (radius ∝ usage count), edges with stroke-opacity ∝ weight,
   * and the `tags · links · assets` header counts. Ring layout only — no physics, no deps.
   */
  import Icon from "./Icon.svelte";
  import {
    libraryStats,
    tagGraph,
    fmtSize,
    fmtDuration,
    typeToken,
    typeIcon,
    type Stats,
    type TagGraph,
  } from "./api";
  import { view } from "./nav.svelte";

  let stats = $state<Stats | null>(null);
  let error = $state<string | null>(null);
  let graph = $state<TagGraph | null>(null);
  let graphError = $state<string | null>(null);

  $effect(() => {
    view.dataVersion;
    libraryStats()
      .then((s) => {
        stats = s;
        error = null;
      })
      .catch((e) => (error = String(e)));
    tagGraph()
      .then((g) => {
        graph = g;
        graphError = null;
      })
      .catch((e) => (graphError = String(e)));
  });

  let identified = $derived(
    stats && stats.total_assets > 0
      ? Math.round(((stats.total_assets - stats.unidentified) / stats.total_assets) * 100)
      : 0,
  );
  let heaviestPackShare = $derived(
    stats && stats.bytes_total > 0 && stats.by_pack.length > 0
      ? Math.round((stats.by_pack[0].bytes / stats.bytes_total) * 100)
      : 0,
  );

  // --- Tag Graph layout (UI-SPEC §8.2) ------------------------------------------
  // Deterministic ring: tags walk clockwise from 12 o'clock in backend order (usage-desc), each
  // node's radius scaled to its usage count, co-occurrence edges drawn beneath with opacity scaled
  // to their weight. No physics engine, no dependencies.
  const GRAPH_W = 1000;
  const GRAPH_H = 520;
  const NODE_R_MIN = 6;
  const NODE_R_MAX = 24;
  const LABEL_MAX = 22;

  type GraphPos = {
    id: number;
    x: number;
    y: number;
    r: number;
    label: string;
    title: string;
    anchor: "start" | "end";
    labelX: number;
    labelY: number;
  };
  type GraphEdgePos = {
    x1: number;
    y1: number;
    x2: number;
    y2: number;
    opacity: number;
  };
  type GraphLayout = { nodes: GraphPos[]; edges: GraphEdgePos[] };

  function layoutTagGraph(g: TagGraph): GraphLayout {
    const cx = GRAPH_W / 2;
    const cy = GRAPH_H / 2;
    const maxUsage = g.nodes.reduce((m, n) => Math.max(m, n.usage_count), 0);
    const maxWeight = g.edges.reduce((m, e) => Math.max(m, e.weight), 0);
    const ring = Math.min(GRAPH_W, GRAPH_H) / 2 - NODE_R_MAX - 84;
    const byId = new Map<number, GraphPos>();
    const nodes = g.nodes.map((n, i) => {
      const r =
        maxUsage > 0
          ? NODE_R_MIN + (n.usage_count / maxUsage) * (NODE_R_MAX - NODE_R_MIN)
          : NODE_R_MIN;
      const a = (i / Math.max(1, g.nodes.length)) * Math.PI * 2 - Math.PI / 2;
      const x = cx + Math.cos(a) * ring;
      const y = cy + Math.sin(a) * ring;
      const dir = Math.cos(a) >= 0 ? 1 : -1;
      const label = n.value.length > LABEL_MAX ? n.value.slice(0, LABEL_MAX - 1) + "…" : n.value;
      const pos: GraphPos = {
        id: n.tag_id,
        x,
        y,
        r,
        label,
        title: `${n.namespace}/${n.value} · ${n.usage_count} ${
          n.usage_count === 1 ? "asset" : "assets"
        }`,
        anchor: dir >= 0 ? "start" : "end",
        labelX: Math.max(6, Math.min(GRAPH_W - 6, x + dir * (r + 6))),
        labelY: y,
      };
      byId.set(n.tag_id, pos);
      return pos;
    });
    const edges: GraphEdgePos[] = [];
    for (const e of g.edges) {
      const a = byId.get(e.tag_a);
      const b = byId.get(e.tag_b);
      if (!a || !b) continue;
      edges.push({
        x1: a.x,
        y1: a.y,
        x2: b.x,
        y2: b.y,
        opacity: maxWeight > 0 ? 0.08 + (e.weight / maxWeight) * 0.55 : 0.08,
      });
    }
    return { nodes, edges };
  }

  let layout: GraphLayout = $derived(graph ? layoutTagGraph(graph) : { nodes: [], edges: [] });

  // --- Tag Graph interactivity (UI-SPEC §8.2): pan by dragging, zoom with the wheel, and
  // click a node to filter the library to that tag. ------------------------------
  let zoom = $state(1);
  let pan = $state({ x: 0, y: 0 });
  let drag: { startX: number; startY: number; panX: number; panY: number } | null = null;
  const zoomAt = (e: WheelEvent) => {
    const next = Math.min(4, Math.max(0.5, zoom * Math.exp(-e.deltaY * 0.001)));
    zoom = next;
  };
  const startPan = (e: PointerEvent) => {
    drag = { startX: e.clientX, startY: e.clientY, panX: pan.x, panY: pan.y };
    (e.currentTarget as SVGElement).setPointerCapture(e.pointerId);
  };
  const movePan = (e: PointerEvent) => {
    if (!drag) return;
    pan = { x: drag.panX + e.clientX - drag.startX, y: drag.panY + e.clientY - drag.startY };
  };
  const endPan = () => (drag = null);

  /** Click-through: filter the library to this tag (the same chip the ⚑ popover would add). */
  function openTag(label: string) {
    view.filter = { ...view.filter, traits: [label] };
    view.go("library");
  }

  /** Reveal an asset in the library, the same move the palette makes. */
  function reveal(id: number, name: string) {
    view.go("library");
    view.setSearch(name);
    view.selected = id;
  }
</script>

<section class="screen">
  <header><h1>Stats</h1></header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !stats}
      <p class="msg">Loading…</p>
    {:else}
      <div class="tiles" data-testid="stat-tiles">
        <div class="tile">
          <span class="value num">{stats.total_assets.toLocaleString()}</span>
          <span class="label">assets indexed</span>
          <span class="sub">{stats.sources} libraries · {stats.packs.toLocaleString()} packs</span>
        </div>
        <div class="tile gold">
          <span class="value num">{fmtSize(stats.bytes_total)}</span>
          <span class="label">on disk</span>
          <span class="sub">
            avg {stats.total_assets > 0 ? fmtSize(stats.bytes_total / stats.total_assets) : "—"}
          </span>
        </div>
        <div class="tile violet">
          <span class="value num">{stats.triangles_total.toLocaleString()}</span>
          <span class="label">triangles</span>
          <span class="sub">{stats.models.toLocaleString()} models</span>
        </div>
        <div class="tile" class:amber={identified < 100} class:verdant={identified === 100}>
          <span class="value num">{identified}%</span>
          <span class="label">identified</span>
          <span class="sub">{stats.unidentified.toLocaleString()} unidentified</span>
        </div>
      </div>

      <h2>Size by type</h2>
      <div class="bars">
        {#each stats.by_type as t (t.media_type)}
          <div class="bar-row">
            <span class="bar-label" style="--tint: var({typeToken(t.media_type)})">
              <Icon name={typeIcon(t.media_type)} size={14} />
              {t.media_type}
            </span>
            <div class="track">
              <div
                class="fill"
                style="--tint: var({typeToken(t.media_type)}); width: {stats.bytes_total > 0
                  ? (t.bytes / stats.bytes_total) * 100
                  : 0}%"
              ></div>
            </div>
            <span class="bar-meta num">
              {fmtSize(t.bytes)} · {t.count.toLocaleString()}
              {#if t.triangles > 0}· {t.triangles.toLocaleString()} tris{/if}
              {#if t.duration_ms > 0}· {fmtDuration(t.duration_ms)}{/if}
            </span>
          </div>
        {/each}
      </div>

      {#if stats.by_pack.length > 0}
        <h2>Disk weight by pack</h2>
        <div class="bars">
          {#each stats.by_pack as p (p.name)}
            <div class="bar-row">
              <span class="bar-label pack" title={p.name}>{p.name}</span>
              <div class="track">
                <div
                  class="fill gold"
                  style="width: {(p.bytes / stats.by_pack[0].bytes) * 100}%"
                ></div>
              </div>
              <span class="bar-meta num">{fmtSize(p.bytes)} · {p.count.toLocaleString()}</span>
            </div>
          {/each}
        </div>
      {/if}

      <h2>Heaviest assets</h2>
      <ul class="heaviest">
        {#each stats.heaviest as a (a.asset_id)}
          <li>
            <button onclick={() => reveal(a.asset_id, a.name)}>
              <span class="name">{a.name}</span>
              <span class="meta num">{a.package ?? "on disk"} · {fmtSize(a.size_bytes)}</span>
            </button>
          </li>
        {/each}
      </ul>

      <h2>Insights</h2>
      <ul class="insights" data-testid="insights">
        {#if stats.unidentified > 0}
          <li>
            <span class="glyph amber">?</span>
            {stats.unidentified.toLocaleString()} assets are Unidentified — invisible to AI search
            until appraised.
            <button class="link" onclick={() => view.go("identify")}>Open Identify</button>
          </li>
        {/if}
        {#if stats.duplicate_groups > 0}
          <li>
            <span class="glyph">⧉</span>
            {stats.duplicate_groups.toLocaleString()} exact-duplicate groups are waiting for review.
            <button class="link" onclick={() => view.go("dups")}>Open Duplicates</button>
          </li>
        {/if}
        {#if heaviestPackShare > 0}
          <li>
            <span class="glyph gold">▤</span>
            {stats.by_pack[0].name} is {heaviestPackShare}% of your library's disk.
          </li>
        {/if}
      </ul>

      <h2>Tag graph</h2>
      <div class="graph-counts num" data-testid="tag-graph-counts">
        {graph ? graph.nodes.length : "—"} tags · {graph ? graph.link_count : "—"} links · {graph
          ? graph.asset_count
          : "—"} assets
      </div>

      {#if graphError}
        <p class="msg danger">{graphError}</p>
      {:else if !graph}
        <p class="msg">Loading…</p>
      {:else if graph.nodes.length === 0}
        <div class="graph-empty" data-testid="tag-graph-empty">
          <p class="line">No tags yet.</p>
          <p class="sub">
            Assets you tag — or the identifier emits on scan — appear here as gold nodes; the wider
            a node, the more assets carry that tag.
          </p>
        </div>
      {:else}
        <svg
          class="tag-graph"
          data-testid="tag-graph"
          viewBox="0 0 {GRAPH_W} {GRAPH_H}"
          role="img"
          aria-label="Tag co-occurrence graph — drag to pan, wheel to zoom, click a tag to filter the library"
          onwheel={(e) => {
            e.preventDefault();
            zoomAt(e);
          }}
          onpointerdown={startPan}
          onpointermove={movePan}
          onpointerup={endPan}
          onpointerleave={endPan}
        >
          <g transform="translate({pan.x} {pan.y}) scale({zoom})">
            <g class="edges">
              {#each layout.edges as e (`${e.x1}:${e.y1}:${e.x2}:${e.y2}`)}
                <line
                  class="edge"
                  x1={e.x1}
                  y1={e.y1}
                  x2={e.x2}
                  y2={e.y2}
                  stroke-opacity={e.opacity}
                />
              {/each}
            </g>
            <g class="nodes">
              {#each layout.nodes as p (p.id)}
                <g
                  class="node"
                  transform="translate({p.x} {p.y})"
                  onclick={() => openTag(p.label)}
                  role="button"
                  tabindex="0"
                  onkeydown={(e) => {
                    if (e.key === "Enter") openTag(p.label);
                  }}
                >
                  <title>{p.title}</title>
                  <circle r={p.r} />
                  <text
                    class="label"
                    x={p.labelX - p.x}
                    y={p.labelY - p.y}
                    text-anchor={p.anchor}
                    dy="0.35em"
                  >
                    {p.label}
                  </text>
                </g>
              {/each}
            </g>
          </g>
        </svg>
      {/if}

      {#if stats.unidentified > 0}
        <p class="msg amber" data-testid="tag-graph-unidentified-note">
          ? {stats.unidentified.toLocaleString()} Unidentified assets have no tags yet — they're
          not in the graph.
        </p>
      {/if}
    {/if}
  </div>
</section>

<style>
  .screen {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
  }
  header {
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
  }
  h1 {
    font-size: var(--fs-2xl);
  }
  h2 {
    margin: var(--sp-5) 0 var(--sp-2);
    font-size: var(--fs-xs);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ls-text-dim);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 var(--sp-5) var(--sp-5);
  }
  .msg {
    color: var(--ls-text-dim);
  }
  .msg.danger {
    color: var(--ls-danger);
  }

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: var(--sp-3);
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--sp-3);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .tile .value {
    font-size: var(--fs-2xl);
    color: var(--ls-text);
  }
  .tile.gold .value {
    color: var(--ls-gold);
  }
  .tile.violet .value {
    color: var(--ls-type-model);
  }
  .tile.amber .value {
    color: var(--ls-warning);
  }
  .tile.verdant .value {
    color: var(--ls-success, var(--ls-status-confirmed));
  }
  .tile .label {
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .tile .sub {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }

  .bars {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .bar-row {
    display: grid;
    grid-template-columns: 10rem minmax(0, 1fr) max-content;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
  }
  .bar-label {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--tint, var(--ls-text-dim));
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bar-label.pack {
    color: var(--ls-text-dim);
  }
  .track {
    height: 8px;
    background: var(--ls-bg);
    border-radius: var(--r-pill);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--tint, var(--ls-gold));
    border-radius: var(--r-pill);
  }
  .fill.gold {
    background: var(--ls-gold);
  }
  .bar-meta {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }

  .heaviest,
  .insights {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .heaviest button {
    display: flex;
    width: 100%;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border-radius: var(--r-sm);
    background: none;
    border: none;
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
    text-align: left;
    cursor: pointer;
  }
  .heaviest button:hover {
    background: var(--ls-overlay);
  }
  .heaviest .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .heaviest .meta {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }

  .insights li {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
  }
  .glyph {
    color: var(--ls-text-muted);
  }
  .glyph.amber {
    color: var(--ls-warning);
  }
  .glyph.gold {
    color: var(--ls-gold);
  }
  .link {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--ls-gold);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
    text-decoration: underline;
  }

  /* Tag Graph (§8.2) */
  .graph-counts {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    margin-bottom: var(--sp-2);
  }
  .tag-graph:active {
    cursor: grabbing;
  }
  .tag-graph {
    display: block;
    width: 100%;
    height: auto;
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    cursor: grab;
    touch-action: none;
    border-radius: var(--r-md);
  }
  .tag-graph .edge {
    stroke: var(--ls-text-dim);
    stroke-width: 1;
  }
  .tag-graph .node {
    cursor: pointer;
  }
  .tag-graph .node circle {
    fill: var(--ls-gold);
    stroke: color-mix(in srgb, var(--ls-gold) 55%, var(--ls-bg));
    stroke-width: 1;
  }
  .tag-graph .node:hover circle {
    fill: var(--ls-gold);
    filter: brightness(1.15);
  }
  .tag-graph .node .label {
    fill: var(--ls-text-dim);
    font-family: var(--ff-ui);
    font-size: var(--fs-xs);
    pointer-events: none;
  }
  .graph-empty {
    padding: var(--sp-4) var(--sp-3);
    background: var(--ls-surface);
    border: 1px dashed var(--ls-border-strong);
    border-radius: var(--r-md);
    text-align: center;
  }
  .graph-empty .line {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text);
  }
  .graph-empty .sub {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  .msg.amber {
    color: var(--ls-warning);
  }
</style>
