<script lang="ts">
  /**
   * Settings (UI-SPEC §11, design README §5): a scrollable column of section cards, max-width
   * 760px, on `--ls-surface` with `--ls-border` and 10px radius. Everything persists through
   * `AppConfig` (config.json) via the whole-object `get_settings`/`set_settings` snapshot.
   *
   * Honest-state rules: the MCP card configures the allow-list the engine build honours (B7);
   * the engine bridge and Clear-cache actions bind when this shell hosts the engine; the
   * per-source blacklist panel needs the Sources-manager slice (documented in ALIGNMENT-AUDIT).
   */
  import { getSettings, setSettings, setSourceWatch, libraryStatus, type Settings, type LibraryStatus } from "./api";
  import { view } from "./nav.svelte";

  const DEFAULTS: Settings = {
    m_kb: 1024,
    l_kb: 4096,
    xl_kb: 8192,
    cap_kb: 10_240,
    preview_quality: "low",
    skip_onboarding: false,
    watch_sources: true,
    auto_identify: false,
    cache_extracted: false,
    preview_cache_cap_gb: 4,
    auto_purge_preview_cache: false,
    mcp_disabled_tools: [],
  };

  let s = $state<Settings>({ ...DEFAULTS });
  let loaded = $state(false);
  let status = $state<LibraryStatus | null>(null);
  let error = $state<string | null>(null);
  let saved = $state(false);

  const QUALITY_TILES = [
    { id: "low", label: "Low", note: "fast · pixelated" },
    { id: "medium", label: "Medium", note: "balanced" },
    { id: "high", label: "High", note: "sharp · heavier GPU" },
  ] as const;

  const TIERS = [
    { id: "integrated", label: "Integrated", note: "on-board graphics · small models" },
    { id: "balanced", label: "Balanced", note: "mid-range discrete GPU" },
    { id: "dedicated", label: "Dedicated GPU", note: "large VRAM · big models" },
  ] as const;

  /** The S/M/L/XL boundary fields — keys of `Settings`, so the input binds straight to it. */
  const TIER_FIELDS: { key: "m_kb" | "l_kb" | "xl_kb"; label: string }[] = [
    { key: "m_kb", label: "S | M" },
    { key: "l_kb", label: "M | L" },
    { key: "xl_kb", label: "L | XL" },
  ];

  /** The canonical MCP tools, grouped as the allow-list presents them (UI-SPEC §11.5). */
  const MCP_GROUPS: { id: string; label: string; tools: string[] }[] = [
    { id: "identify_asset", label: "identify_asset", tools: ["identify_asset"] },
    { id: "tag_trait", label: "add_tag / retag", tools: ["add_tag", "retag"] },
    { id: "search_library", label: "search_assets / get_asset", tools: ["search_assets", "get_asset"] },
  ];

  $effect(() => {
    getSettings()
      .then((g) => {
        s = g;
        loaded = true;
      })
      .catch((e) => (error = String(e)));
    libraryStatus()
      .then((st) => (status = st))
      .catch(() => {});
  });

  async function save() {
    saved = false;
    try {
      await setSettings(s);
      saved = true;
      setTimeout(() => (saved = false), 1500);
      // The size tiers feed the badges and the ⚑ size facet — re-ask so every screen follows.
      view.dataVersion += 1;
    } catch (e) {
      error = String(e);
    }
  }

  function tierQuality(tier: (typeof TIERS)[number]["id"]): Settings["preview_quality"] {
    return tier === "integrated" ? "low" : tier === "balanced" ? "medium" : "high";
  }

  /** Clamp one boundary, enforcing the 128 KB gap and the [128, cap] range (UI-SPEC §11.4). */
  function fixBoundaries() {
    if (!s) return;
    const gap = 128;
    const cap = Math.max(s.cap_kb, s.xl_kb + gap);
    s.cap_kb = Math.min(cap, 10_240);
    s.xl_kb = Math.min(Math.max(s.xl_kb, s.l_kb + gap), Math.max(s.cap_kb - gap, 128));
    s.l_kb = Math.min(Math.max(s.l_kb, s.m_kb + gap), Math.max(s.xl_kb - gap, 128));
    s.m_kb = Math.min(Math.max(s.m_kb, 128), Math.max(s.l_kb - gap, 128));
  }

  function resetTiers() {
    if (!s) return;
    s.m_kb = DEFAULTS.m_kb;
    s.l_kb = DEFAULTS.l_kb;
    s.xl_kb = DEFAULTS.xl_kb;
    s.cap_kb = DEFAULTS.cap_kb;
    save();
  }

  let sourceBusy = $state<number | null>(null);

  async function toggleWatch(id: number, watch: boolean) {
    sourceBusy = id;
    try {
      await setSourceWatch(id, watch);
      view.dataVersion += 1;
      status = await libraryStatus();
    } catch (e) {
      error = String(e);
    } finally {
      sourceBusy = null;
    }
  }
</script>

<section class="screen">
  <header>
    <h1>Settings</h1>
    <p class="subtitle">
      Sources, the identify profile matched to your hardware, storage caps, and the live engine
      bridge. Everything here persists in config.json.
    </p>
    {#if saved}
      <span class="saved" data-testid="settings-saved">saved</span>
    {/if}
  </header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !loaded}
      <p class="msg">Loading…</p>
    {:else}
      <section class="card">
        <h2>WATCHED SOURCES</h2>
        <p class="hint">
          Paused sources stay cataloged but stop being watched for changes. Drop a folder on the
          window to add a source.
        </p>
        <ul class="sources" data-testid="sources">
          {#each status?.sources ?? [] as src (src.id)}
            <li class="src">
              <span class="dot" class:paused={!src.watch_enabled} aria-hidden="true"></span>
              <div class="meta">
                <span class="name">{src.name}</span>
                <span class="path mono" title={src.root_path}>{src.root_path}</span>
                <span class="stat mono">{src.asset_count.toLocaleString()} assets</span>
              </div>
              <button
                class="toggle"
                class:on={src.watch_enabled}
                disabled={sourceBusy === src.id}
                onclick={() => toggleWatch(src.id, !src.watch_enabled)}
                aria-pressed={src.watch_enabled}
                data-testid="src-watch-{src.id}"
              >
                {src.watch_enabled ? "watching" : "paused"}
              </button>
            </li>
          {/each}
        </ul>
      </section>

      <section class="card">
        <h2>IDENTIFY PROFILE</h2>
        <p class="hint">
          The tier sets the local model set and the on-screen quality it recommends
          (AI-MODELS-BY-GPU). Choosing one also sets ON-SCREEN QUALITY below.
        </p>
        <div class="tiles">
          {#each TIERS as t (t.id)}
            <button
              class="tile"
              class:on={s.preview_quality === tierQuality(t.id)}
              onclick={() => {
                s.preview_quality = tierQuality(t.id);
                save();
              }}
            >
              <span class="tname">{t.label}</span>
              <span class="tnote">{t.note}</span>
            </button>
          {/each}
        </div>
        <label class="check">
          <input type="checkbox" bind:checked={s.auto_identify} onchange={save} data-testid="auto-identify" />
          Auto-identify new files — watched-source drops run identification automatically instead
          of only landing in the backlog.
        </label>
      </section>

      <section class="card">
        <h2>ON-SCREEN QUALITY</h2>
        <p class="hint">
          How sharply the grid thumbnails render — a GPU/perf tradeoff. Grid-only: Quick Look
          always renders the full detail.
        </p>
        <div class="tiles">
          {#each QUALITY_TILES as t (t.id)}
            <button
              class="tile"
              class:on={s.preview_quality === t.id}
              onclick={() => {
                s.preview_quality = t.id;
                save();
              }}
              data-testid="quality-{t.id}"
            >
              <span class="tname">{t.label}</span>
              <span class="tnote">{t.note}</span>
            </button>
          {/each}
        </div>
      </section>

      <section class="card">
        <h2>FILE-SIZE LABELS</h2>
        <p class="hint">
          The thresholds behind the S / M / L / XL badge on every thumbnail and the ⚑ File-size
          filter. Boundaries keep a 128 KB gap; the cap can never sit below the XL threshold.
        </p>
        <div class="tiers">
          {#each TIER_FIELDS as f (f.key)}
            <label class="tier-field">
              <span class="mono">{f.label}</span>
              <input
                class="mono num"
                type="number"
                min={128}
                max={10_240}
                bind:value={s[f.key]}
                onblur={() => {
                  fixBoundaries();
                  save();
                }}
                onkeydown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
                data-testid="tier-{f.key}"
              />
              <span class="unit">KB</span>
            </label>
          {/each}
          <label class="tier-field cap">
            <span class="mono">cap</span>
            <input
              class="mono num"
              type="number"
              min={512}
              max={10_240}
              bind:value={s.cap_kb}
              onblur={() => {
                fixBoundaries();
                save();
              }}
              onkeydown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
              data-testid="tier-cap"
            />
            <span class="unit">KB</span>
          </label>
        </div>
        <button class="ghost" onclick={resetTiers} data-testid="tier-reset">Reset defaults</button>
      </section>

      <section class="card">
        <h2>MCP</h2>
        <p class="hint">
          The per-tool allow-list for the engine build's MCP server (MCP ↔ REST stay 1:1). This
          shell hosts the MCP plane from the B7 cutover — the toggles configure it now.
        </p>
        <ul class="tools">
          {#each MCP_GROUPS as g (g.id)}
            {@const on = g.tools.every((t) => !s.mcp_disabled_tools.includes(t))}
            <li>
              <label class="check">
                <input
                  type="checkbox"
                  checked={on}
                  onchange={(e) => {
                    const checked = (e.currentTarget as HTMLInputElement).checked;
                    s.mcp_disabled_tools = checked
                      ? s.mcp_disabled_tools.filter((t) => !g.tools.includes(t))
                      : [...new Set([...s.mcp_disabled_tools, ...g.tools])];
                    save();
                  }}
                  data-testid="mcp-{g.id}"
                />
                <span class="mono">{g.label}</span>
              </label>
            </li>
          {/each}
        </ul>
        <p class="hint">Status: offline in this build — the server binds with the engine (B7).</p>
      </section>

      <section class="card">
        <h2>STORAGE CAPS</h2>
        <label class="check">
          <input type="checkbox" bind:checked={s.cache_extracted} onchange={save} data-testid="keep-extracted" />
          Keep extracted packs cached — rehydrate from the workspace at startup instead of
          re-extracting.
        </label>
        <div class="slider-row">
          <span class="mono">Preview cache cap</span>
          <input
            class="range"
            type="range"
            min={1}
            max={16}
            step={1}
            bind:value={s.preview_cache_cap_gb}
            onchange={save}
            data-testid="cache-cap"
          />
          <span class="mono num">{s.preview_cache_cap_gb} GB</span>
        </div>
        <label class="check">
          <input
            type="checkbox"
            bind:checked={s.auto_purge_preview_cache}
            onchange={save}
            data-testid="auto-purge"
          />
          Auto-purge over cap — drop least-recently-viewed previews.
        </label>
      </section>

      <section class="card">
        <h2>ENGINE BRIDGE</h2>
        <p class="hint">
          The live bridge to a running engine — port + token handshake — binds when this shell
          hosts the engine API (B7). Until then: <span class="mono">Bridge paused</span>.
        </p>
        <p class="bridge" data-testid="bridge-state">
          <span class="dot paused" aria-hidden="true"></span> Bridge paused — the engine bridge
          ships with the B7 cutover.
        </p>
      </section>

      <section class="card">
        <h2>FIRST RUN</h2>
        <label class="check">
          <input
            type="checkbox"
            checked={!s.skip_onboarding}
            onchange={(e) => {
              s.skip_onboarding = !(e.currentTarget as HTMLInputElement).checked;
              save();
            }}
            data-testid="show-onboarding"
          />
          Show first-run onboarding on launch — "Point it at your hoard".
        </label>
      </section>
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
    display: flex;
    align-items: baseline;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  h1 {
    font-size: var(--fs-2xl);
    margin: 0;
  }
  .subtitle {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    max-width: 70ch;
  }
  .saved {
    margin-left: auto;
    font-family: var(--ff-mono);
    font-size: var(--fs-xs);
    color: var(--ls-success);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 var(--sp-5) var(--sp-6);
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
    max-width: 760px;
  }
  .card {
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-lg);
    padding: var(--sp-4);
  }
  .card h2 {
    margin: 0 0 var(--sp-1);
    font-family: var(--ff-mono);
    font-size: var(--fs-xs);
    font-weight: 600;
    letter-spacing: 0.1em;
    color: var(--ls-text-dim);
  }
  .hint {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .mono {
    font-family: var(--ff-mono);
    font-variant-numeric: tabular-nums;
  }
  .num {
    font-variant-numeric: tabular-nums;
  }

  .sources {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .src {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    background: var(--ls-bg);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ls-success);
    flex: none;
    box-shadow: 0 0 6px var(--ls-success);
  }
  .dot.paused {
    background: var(--ls-text-dim);
    box-shadow: none;
  }
  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .name {
    font-weight: 600;
  }
  .path {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .stat {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  .toggle {
    flex: none;
    padding: 3px var(--sp-3);
    background: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
    color: var(--ls-text-dim);
    font: inherit;
    font-size: var(--fs-xs);
    cursor: pointer;
  }
  .toggle.on {
    color: var(--ls-success);
    border-color: color-mix(in srgb, var(--ls-success) 50%, transparent);
  }
  .toggle:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: var(--sp-2);
    margin-bottom: var(--sp-3);
  }
  .tile {
    padding: var(--sp-3);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text-muted);
    font: inherit;
    cursor: pointer;
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .tile:hover {
    border-color: var(--ls-gold);
  }
  .tile.on {
    border-color: var(--ls-gold);
    background: color-mix(in srgb, var(--ls-gold) 10%, var(--ls-bg));
    color: var(--ls-text);
  }
  .tname {
    font-weight: 600;
  }
  .tnote {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }

  .check {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    cursor: pointer;
    margin-top: var(--sp-2);
  }
  .check input {
    margin: 2px 0 0;
    accent-color: var(--ls-gold);
  }

  .tiers {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-3);
    align-items: end;
    margin-bottom: var(--sp-3);
  }
  .tier-field {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .tier-field input {
    width: 90px;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font-size: var(--fs-sm);
  }
  .tier-field input:focus {
    outline: none;
    border-color: var(--ls-gold);
  }
  .unit {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .tier-field.cap {
    margin-left: var(--sp-2);
  }
  .ghost {
    background: none;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    color: var(--ls-text-muted);
    font: inherit;
    font-size: var(--fs-xs);
    cursor: pointer;
    padding: 3px var(--sp-2);
  }
  .ghost:hover {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }

  .tools {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .tools .check {
    margin-top: 0;
    padding: var(--sp-1) 0;
  }

  .slider-row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin: var(--sp-3) 0 var(--sp-1);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .range {
    flex: 1;
    accent-color: var(--ls-gold);
  }

  .bridge {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .bridge .dot {
    background: var(--ls-text-dim);
    box-shadow: none;
  }

  .msg {
    color: var(--ls-text-dim);
    margin: 0;
  }
  .msg.danger {
    color: var(--ls-danger);
  }
</style>
