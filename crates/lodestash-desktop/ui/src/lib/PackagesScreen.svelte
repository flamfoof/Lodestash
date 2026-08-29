<script lang="ts">
  import EmptyState from "./EmptyState.svelte";
  /**
   * Packages (UI-SPEC §4): everything you own, publisher → pack.
   *
   * Counts come from the **indexed rows**, not `package.asset_count` — that column is populated
   * for 1 of 87 packs on a real Asset Store cache, so printing it would render a screen of blanks
   * next to packs that plainly contain things. The number shown is the number Browse → lands on.
   *
   * Packs with nothing indexed are kept, showing `0`: most of a download cache is archives nobody
   * has opened, and hiding them would claim the library holds six packs when it holds eighty-seven.
   * Extraction is not offered here — it needs the ingest path this shell does not have yet (B2b).
   */
  import Icon from "./Icon.svelte";
  import {
    packages,
    fmtSize,
    setPackKeepCached,
    pickPackageFile,
    retargetPreview,
    retargetPackage,
    type PackageRow,
  } from "./api";
  import { view } from "./nav.svelte";

  let rows = $state<PackageRow[] | null>(null);
  let error = $state<string | null>(null);
  let q = $state("");

  $effect(() => {
    view.dataVersion;
    packages()
      .then((r) => {
        rows = r;
        error = null;
      })
      .catch((e) => (error = String(e)));
  });

  let shown = $derived(
    (rows ?? []).filter((p) => {
      const needle = q.trim().toLowerCase();
      return (
        needle === "" ||
        p.name.toLowerCase().includes(needle) ||
        p.publisher.toLowerCase().includes(needle)
      );
    }),
  );

  /** Publisher → its packs, indexed packs first so the ones you can actually browse lead. */
  let byPublisher = $derived.by(() => {
    const map = new Map<string, PackageRow[]>();
    for (const p of shown) {
      const list = map.get(p.publisher) ?? [];
      list.push(p);
      map.set(p.publisher, list);
    }
    return [...map.entries()]
      .map(([publisher, packs]) => ({
        publisher,
        packs: packs.slice().sort((a, b) => b.count - a.count || a.name.localeCompare(b.name)),
        total: packs.reduce((a, p) => a + p.count, 0),
      }))
      .sort((a, b) => b.total - a.total || a.publisher.localeCompare(b.publisher));
  });

  /**
   * §15's keep-cached mark, offered per row so a column of them reads down the list. Marking
   * starts no work: it promises the bytes survive if the pack is extracted, nothing more.
   */
  let keepBusy = $state<number | null>(null);
  async function toggleKeep(p: PackageRow) {
    if (keepBusy !== null) return;
    keepBusy = p.id;
    try {
      await setPackKeepCached(p.id, !p.keep_cached);
      rows = (rows ?? []).map((r) => (r.id === p.id ? { ...r, keep_cached: !r.keep_cached } : r));
    } catch (e) {
      error = String(e);
    } finally {
      keepBusy = null;
    }
  }

  /**
   * Retarget… — point a pack at a `.unitypackage` that has moved (UI-SPEC §15).
   *
   * The GUID match count is **evidence, not a verdict**: a publisher updating a pack legitimately
   * reshuffles GUIDs, so a low match is reported and offered rather than refused. Same shape as
   * the source relink in Settings (`relink_preview`), which is the pattern this follows on
   * purpose — a user who has relinked a moved drive already knows how to read this.
   */
  let retargeting = $state<PackageRow | null>(null);
  let candidate = $state<string | null>(null);
  let match = $state<{ matched: number; total: number } | null>(null);
  let retargetBusy = $state(false);
  let retargetError = $state<string | null>(null);

  async function chooseArchive(p: PackageRow) {
    retargeting = p;
    candidate = null;
    match = null;
    retargetError = null;
    try {
      const picked = await pickPackageFile(`Find “${p.name}”`);
      if (!picked) return;
      candidate = picked;
      retargetBusy = true;
      match = await retargetPreview(p.id, picked);
    } catch (e) {
      retargetError = String(e);
    } finally {
      retargetBusy = false;
    }
  }

  async function confirmRetarget() {
    const p = retargeting;
    if (!p || !candidate || retargetBusy) return;
    retargetBusy = true;
    try {
      await retargetPackage(p.id, candidate);
      rows = await packages();
      retargeting = null;
      candidate = null;
      match = null;
    } catch (e) {
      retargetError = String(e);
    } finally {
      retargetBusy = false;
    }
  }

  function browse(pack: PackageRow) {
    view.packageFilter = pack.name;
    view.go("library");
  }
</script>

<section class="screen">
  <header>
    <h1>Packages</h1>
    <input class="search" placeholder="Filter packs and publishers" bind:value={q} />
  </header>

  <div class="body">
    {#if error}
      <p class="msg danger">{error}</p>
    {:else if !rows}
      <p class="msg">Loading…</p>
    {:else if byPublisher.length === 0}
      {#if q}
        <!-- Filtered-empty is not empty: it needs a way out, not a lesson about what a pack
             is (UI-SPEC §2.6). -->
        <p class="msg" data-testid="empty">No pack matches “{q}”.</p>
      {:else}
        <EmptyState
          testid="empty"
          icon="ls-nav-packages"
          title="No packs indexed"
          line="A pack is one publisher's release — Synty's Dungeon Realms, say. They appear here once a source is scanned."
        />
      {/if}
    {:else}
      <div class="publishers" data-testid="publishers">
        {#each byPublisher as group (group.publisher)}
          <section class="publisher">
            <h2>
              <Icon name="ls-nav-packages" size={16} />
              {group.publisher}
              <span class="count num">{group.total.toLocaleString()} assets</span>
            </h2>
            <ul>
              {#each group.packs as p (p.id)}
                <li class="pack" class:empty={p.count === 0}>
                  <button
                    class="keep"
                    class:on={p.keep_cached}
                    onclick={() => toggleKeep(p)}
                    disabled={keepBusy !== null}
                    aria-pressed={p.keep_cached}
                    title={p.keep_cached
                      ? "Kept — extracted bytes stay on disk"
                      : "Evictable — reclaimed once nothing references it"}
                    data-testid="pack-keep-{p.id}"
                  >
                    <Icon name="ls-nav-packages" size={14} />
                  </button>
                  <span class="name" title={p.category ?? ""}>{p.name}</span>
                  {#if p.unidentified > 0}
                    <span class="pill unid num">{p.unidentified.toLocaleString()} unid</span>
                  {/if}
                  {#if p.extracted}
                    <span class="pill">extracted</span>
                  {/if}
                  <span class="stat num">
                    {p.count.toLocaleString()}
                    {#if p.bytes > 0}· {fmtSize(p.bytes)}{/if}
                  </span>
                  {#if !p.archive_present}
                    <span class="pill gone">archive missing</span>
                    <button
                      class="browse"
                      onclick={() => chooseArchive(p)}
                      data-testid="pack-retarget-{p.id}"
                    >
                      Retarget…
                    </button>
                  {:else if p.count > 0}
                    <button class="browse" onclick={() => browse(p)} data-testid="pack-browse">
                      Browse →
                    </button>
                  {:else}
                    <span class="not-indexed">not indexed</span>
                  {/if}
                </li>
              {/each}
            </ul>
          </section>
        {/each}
      </div>
    {/if}
  </div>
</section>

<!--
  Retarget… (UI-SPEC §15). Deliberately the same shape as the source relink in Settings: pick a
  candidate, see how much of the pack it accounts for, then decide. The count is evidence, never
  a verdict — a publisher updating a pack legitimately reshuffles GUIDs, so a poor match is
  offered rather than refused.
-->
{#if retargeting}
  {@const pack = retargeting}
  <div
    class="scrim"
    role="button"
    tabindex="-1"
    onclick={() => (retargeting = null)}
    onkeydown={(e) => e.key === "Escape" && (retargeting = null)}
  >
    <div
      class="sheet"
      role="dialog"
      aria-modal="true"
      aria-label="Retarget {pack.name}"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
      tabindex="-1"
      data-testid="retarget-sheet"
    >
      <h3>{pack.name}</h3>
      <p class="declared mono">{pack.package_path}</p>
      <p class="hint">
        The archive is gone from that path. Its {pack.count.toLocaleString()} indexed assets still
        resolve from whatever was extracted — nothing more can be pulled until it is found, and
        this pack is never reclaimed while its extracted copy is the only one.
      </p>

      {#if candidate}
        <div class="candidate" class:poor={match !== null && match.matched < match.total}>
          <span class="mono path">{candidate}</span>
          {#if retargetBusy && match === null}
            <span class="count">counting…</span>
          {:else if match}
            <span class="count" data-testid="retarget-count">
              {match.matched.toLocaleString()} of {match.total.toLocaleString()} entries matched by
              GUID
            </span>
          {/if}
        </div>
      {/if}
      {#if retargetError}
        <p class="err">{retargetError}</p>
      {/if}

      <div class="acts">
        <button class="ghost" onclick={() => chooseArchive(pack)} disabled={retargetBusy}>
          {candidate ? "Choose another…" : "Find the archive…"}
        </button>
        <button
          class="primary"
          onclick={confirmRetarget}
          disabled={!candidate || retargetBusy}
          data-testid="retarget-confirm"
        >
          Retarget to this file
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .screen {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-4) var(--sp-5) var(--sp-3);
  }
  h1 {
    font-size: var(--fs-2xl);
  }
  .search {
    margin-left: auto;
    width: min(280px, 40%);
    padding: var(--sp-1) var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
    font-size: var(--fs-sm);
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

  .publishers {
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-1);
    font-size: var(--fs-md);
    color: var(--ls-gold);
  }
  h2 .count {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .pack {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border-top: 1px solid var(--ls-border);
    font-size: var(--fs-sm);
  }
  .pack.empty {
    color: var(--ls-text-muted);
  }
  .pack .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill {
    padding: 0 var(--sp-1);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .pill.unid {
    border-color: var(--ls-warning);
    color: var(--ls-warning);
  }
  .stat {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }
  .browse {
    background: none;
    border: none;
    color: var(--ls-gold);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .browse:hover {
    text-decoration: underline;
  }
  .pill.gone {
    border-color: var(--ls-danger);
    color: var(--ls-danger);
  }

  /* Retarget sheet (§15) — the pack-level twin of Settings' source relink. */
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.6);
    display: grid;
    place-items: center;
    z-index: 100;
  }
  .sheet {
    width: min(520px, 92vw);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    padding: var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    box-shadow: var(--el-popover-shadow);
  }
  .sheet h3 {
    margin: 0;
    font-size: var(--fs-lg);
  }
  .declared {
    margin: 0;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    word-break: break-all;
    user-select: text;
  }
  .sheet .hint {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .candidate {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    background: var(--ls-surface-raised);
    border: 1px solid var(--ls-border-strong);
    border-left: 2px solid var(--ls-success);
    border-radius: var(--r-md);
  }
  .candidate.poor {
    border-left-color: var(--ls-warning);
  }
  .candidate .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
  .candidate .count {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--ls-success);
  }
  .candidate.poor .count {
    color: var(--ls-warning);
  }
  .sheet .err {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-danger);
  }
  .acts {
    display: flex;
    justify-content: flex-end;
    gap: var(--sp-2);
    margin-top: var(--sp-1);
  }
  .acts .ghost,
  .acts .primary {
    font: inherit;
    font-size: var(--fs-xs);
    padding: 3px var(--sp-2);
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  .acts .ghost {
    background: none;
    border: 1px solid var(--ls-border);
    color: var(--ls-text-muted);
  }
  .acts .primary {
    background: color-mix(in srgb, var(--ls-gold) 18%, transparent);
    border: 1px solid var(--ls-gold);
    color: var(--ls-gold-bright);
  }
  .acts button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* The keep-cached mark. The sprite bakes its own stroke colour (Icon.svelte), so an
     evictable pack is dimmed with opacity rather than tinted. */
  .keep {
    flex: none;
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    padding: 0;
    background: none;
    border: none;
    cursor: pointer;
    opacity: 0.3;
  }
  .keep.on {
    opacity: 1;
  }
  .keep:hover:not(:disabled) {
    opacity: 0.75;
  }
  .keep:disabled {
    cursor: default;
  }

  .not-indexed {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
  }
</style>
