<script lang="ts">
  /**
   * Traits (UI-SPEC §9) — the vocabulary, grouped by namespace, most-used first. **A working
   * screen (B8a):** the NEW TRAIT card, the ✦ promote-existing-tags strip, the teaching toggle,
   * per-trait style (6-colour palette), and demote — all through the same writer thread every
   * other catalog write goes through. Chips across the app (⚑ cloud, cards) pick up a trait's
   * colour from the shared `traitColor` map.
   *
   * The screen hosts two tabs (§9): **❖ Traits** — the vocabulary above — and **⇢ Mapping**
   * (§9.2) — the taxonomy/category/rule editor. This build's slice is honest about what it can
   * do: a taxonomy switcher (● active marker, switch + delete, no create button in this slice), a
   * category tree built from the categories the rules target (add root/child, rename, delete with
   * a confirm), a read-only rules list, and an ⚙ Apply button that reports the changed count.
   * The tree is derived from `listRules` because that is the only category read this slice
   * exposes — a category shows up once a rule targets it.
   */
  import {
    vocabulary,
    upsertTrait,
    unpromoteTrait,
    setTraitTeach,
    setTraitStyle,
    taxonomies,
    setActiveTaxonomy,
    deleteTaxonomy,
    addCategory,
    renameCategory,
    deleteCategory,
    listRules,
    applyMapping,
    type VocabRow,
    type TaxonomyRow,
    type MappingRuleRow,
  } from "./api";
  import { refreshTraitColors } from "./traits.svelte";
  import { view } from "./nav.svelte";

  let rows = $state<VocabRow[] | null>(null);
  let error = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let q = $state("");
  let ntName = $state("");
  let ntNamespace = $state("trait");

  const STYLE_SWATCHES = [
    "--ls-type-model",
    "--ls-type-sound",
    "--ls-type-image",
    "--ls-type-material",
    "--ls-type-sprite",
    "--ls-type-font",
  ] as const;

  async function load() {
    try {
      rows = await vocabulary();
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    view.dataVersion;
    load();
  });

  /** Run a write, then re-ask everything the write could have changed. */
  async function write(run: () => Promise<unknown>) {
    actionError = null;
    try {
      await run();
      view.dataVersion += 1;
      await refreshTraitColors();
    } catch (e) {
      actionError = String(e);
    }
  }

  // ---- Mapping tab (UI-SPEC §9.2) -------------------------------------------
  // Read slice: `taxonomies()` feeds the switcher, and `listRules(activeId)` is the only
  // category read this slice exposes — so the category tree below is derived from the
  // categories rules target. All writes go through the same writer thread every catalog write
  // goes through, then bump `view.dataVersion` so every screen re-asks its question.
  let tab = $state<"traits" | "mapping">("traits");
  let taxRows = $state<TaxonomyRow[] | null>(null);
  let rules = $state<MappingRuleRow[] | null>(null);
  let mapError = $state<string | null>(null);
  let mapActionError = $state<string | null>(null);
  let applied = $state<string | null>(null);
  let applying = $state(false);
  let draft = $state("");
  let collapsed = $state<Set<number>>(new Set());

  let activeTaxonomy = $derived(taxRows?.find((t) => t.is_active) ?? null);

  type MappingEdit =
    | { mode: "add-root" }
    | { mode: "add-child"; id: number }
    | { mode: "rename"; id: number };
  let edit = $state<MappingEdit | null>(null);

  type MappingConfirm = { kind: "taxonomy" | "category"; name: string; run: () => Promise<void> };
  let confirm = $state<MappingConfirm | null>(null);

  async function loadMapping() {
    try {
      const ts = await taxonomies();
      taxRows = ts;
      const active = ts.find((t) => t.is_active);
      rules = active ? await listRules(active.id) : [];
      mapError = null;
    } catch (e) {
      mapError = String(e);
    }
  }

  $effect(() => {
    view.dataVersion;
    if (tab === "mapping") loadMapping();
  });

  /** Run a mapping write, re-ask, and clear the stale Apply result. */
  async function mapWrite(run: () => Promise<unknown>) {
    mapActionError = null;
    applied = null;
    try {
      await run();
      view.dataVersion += 1;
    } catch (e) {
      mapActionError = String(e);
    }
  }

  async function switchActive(id: number) {
    if (id === activeTaxonomy?.id) return;
    await mapWrite(() => setActiveTaxonomy(id));
  }

  function askTaxonomyDelete(row: TaxonomyRow) {
    confirm = {
      kind: "taxonomy",
      name: row.name,
      run: async () => {
        confirm = null;
        await mapWrite(() => deleteTaxonomy(row.id));
      },
    };
  }

  function askCategoryDelete(node: CatNode) {
    confirm = {
      kind: "category",
      name: node.path,
      run: async () => {
        confirm = null;
        await mapWrite(() => deleteCategory(node.id));
      },
    };
  }

  async function applyActive() {
    const id = activeTaxonomy?.id;
    if (id == null || applying) return;
    applying = true;
    mapActionError = null;
    try {
      const res = await applyMapping(id);
      const n = res.changed;
      applied =
        n === 0
          ? "Applied — no assets changed."
          : `Applied — ${n.toLocaleString()} ${n === 1 ? "asset" : "assets"} changed.`;
      view.dataVersion += 1;
    } catch (e) {
      mapActionError = String(e);
    } finally {
      applying = false;
    }
  }

  // ---- Category tree, derived from the rules' target categories -------------
  // `list_categories` is not part of this slice's read surface, so the tree is rebuilt from the
  // unique (category_id, category_path) pairs the rules point at. A category with no rule has no
  // row here — the copy says so rather than pretending to be a full taxonomy browser.
  type CatNode = {
    id: number;
    path: string;
    label: string;
    children: CatNode[];
  };

  let catTree = $derived.by<CatNode[]>(() => {
    const byPath = new Map<string, CatNode>();
    for (const r of rules ?? []) {
      if (byPath.has(r.category_path)) continue;
      byPath.set(r.category_path, {
        id: r.category_id,
        path: r.category_path,
        label: r.category_path.split("/").filter(Boolean).at(-1) ?? r.category_path,
        children: [],
      });
    }
    const roots: CatNode[] = [];
    for (const node of byPath.values()) {
      const segs = node.path.split("/").filter(Boolean);
      const parent = segs.length > 1 ? byPath.get(segs.slice(0, -1).join("/")) : undefined;
      if (parent) parent.children.push(node);
      else roots.push(node);
    }
    const sort = (list: CatNode[]) => {
      list.sort((a, b) => a.label.localeCompare(b.label));
      for (const n of list) sort(n.children);
    };
    sort(roots);
    return roots;
  });

  /** Flatten the tree into visible rows, honouring the collapsed set. */
  let treeRows = $derived.by(() => {
    const out: { node: CatNode; depth: number }[] = [];
    const walk = (nodes: CatNode[], depth: number) => {
      for (const n of nodes) {
        out.push({ node: n, depth });
        if (n.children.length > 0 && !collapsed.has(n.id)) walk(n.children, depth + 1);
      }
    };
    walk(catTree, 0);
    return out;
  });

  function findNode(id: number): CatNode | undefined {
    const walk = (nodes: CatNode[]): CatNode | undefined => {
      for (const n of nodes) {
        if (n.id === id) return n;
        const found = walk(n.children);
        if (found) return found;
      }
      return undefined;
    };
    return walk(catTree);
  }

  function toggleCat(id: number) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    collapsed = next;
  }

  function startAddRoot() {
    draft = "";
    edit = { mode: "add-root" };
  }
  function startAddChild(node: CatNode) {
    draft = "";
    edit = { mode: "add-child", id: node.id };
  }
  function startRename(node: CatNode) {
    draft = node.label;
    edit = { mode: "rename", id: node.id };
  }

  async function addRoot() {
    const id = activeTaxonomy?.id;
    const label = draft.trim();
    if (id == null || !label || edit?.mode !== "add-root") return;
    await mapWrite(() => addCategory(id, null, label, label));
    draft = "";
    edit = null;
  }

  async function addChild() {
    const id = activeTaxonomy?.id;
    const label = draft.trim();
    const node = edit?.mode === "add-child" ? findNode(edit.id) : undefined;
    if (id == null || !label || !node) return;
    await mapWrite(() => addCategory(id, node.id, label, `${node.path}/${label}`));
    draft = "";
    edit = null;
  }

  async function submitRename() {
    const label = draft.trim();
    const node = edit?.mode === "rename" ? findNode(edit.id) : undefined;
    if (!label || !node) return;
    await mapWrite(() => renameCategory(node.id, label));
    draft = "";
    edit = null;
  }

  function onConfirmGo() {
    void confirm?.run();
  }

  let shown = $derived(
    (rows ?? []).filter((t) => t.value.toLowerCase().includes(q.trim().toLowerCase())),
  );

  let byNamespace = $derived.by(() => {
    const map = new Map<string, VocabRow[]>();
    for (const t of shown) {
      const list = map.get(t.namespace) ?? [];
      list.push(t);
      map.set(t.namespace, list);
    }
    return [...map.entries()]
      .map(([namespace, traits]) => ({
        namespace,
        traits,
        total: traits.reduce((a, t) => a + t.usage_count, 0),
      }))
      .sort((a, b) => b.total - a.total || a.namespace.localeCompare(b.namespace));
  });

  /** Plain tags — the ✦ promote strip (B8a: `is_trait = false` rows are already listed). */
  let promotable = $derived(shown.filter((t) => !t.is_trait));

  function browse(value: string) {
    view.clearFilter();
    view.filter = { traits: [value], tri_buckets: [], sizes: [] };
    view.go("library");
  }

  async function createTrait() {
    const name = ntName.trim();
    const namespace = ntNamespace.trim() || "trait";
    if (!name) return;
    await write(() => upsertTrait({ namespace, value: name, teach: false }));
    ntName = "";
  }
</script>

<section class="screen">
  <header>
    <h1>Traits</h1>
    <div class="tabs" role="tablist" aria-label="Traits screen sections">
      <button
        class="tab"
        class:on={tab === "traits"}
        role="tab"
        aria-selected={tab === "traits"}
        onclick={() => (tab = "traits")}
        data-testid="tab-traits"
      >
        ❖ Traits
      </button>
      <button
        class="tab"
        class:on={tab === "mapping"}
        role="tab"
        aria-selected={tab === "mapping"}
        onclick={() => (tab = "mapping")}
        data-testid="tab-mapping"
      >
        ⇢ Mapping
      </button>
    </div>
    {#if tab === "traits"}
      <input class="search" placeholder="Filter traits" bind:value={q} />
    {/if}
  </header>

  <div class="body">
    {#if tab === "traits"}
      {#if error}
      <p class="msg danger">{error}</p>
    {:else if !rows}
      <p class="msg">Loading…</p>
    {:else}
      {#if actionError}
        <p class="msg danger">{actionError}</p>
      {/if}

      <section class="card new-trait" data-testid="new-trait">
        <h2>NEW TRAIT</h2>
        <p class="hint">
          A trait is a first-class vocabulary term. Creating one with a name that already exists as
          a tag promotes that tag — its assignments stay.
        </p>
        <div class="fields">
          <input
            placeholder="Name"
            bind:value={ntName}
            data-testid="new-trait-name"
            onkeydown={(e) => e.key === "Enter" && createTrait()}
          />
          <input placeholder="Namespace" bind:value={ntNamespace} aria-label="Namespace" />
          <button class="gold" onclick={createTrait} disabled={!ntName.trim()} data-testid="new-trait-go">
            + Create
          </button>
        </div>
      </section>

      {#if promotable.length > 0}
        <section class="card promote" data-testid="promote-strip">
          <h2>✦ PROMOTE EXISTING TAGS</h2>
          <p class="hint">
            Tags the identifier or you have applied, but that are not yet vocabulary terms.
            Promoting one makes it a trait — the term survives even when no asset carries it.
          </p>
          <div class="chips">
            {#each promotable as t (t.id)}
              <button
                class="chip"
                title="Promote {t.namespace}:{t.value}"
                onclick={() =>
                  write(() => upsertTrait({ namespace: t.namespace, value: t.value, teach: false }))}
                data-testid="promote-{t.value}"
              >
                {t.namespace}:{t.value}
                <span class="count num">{t.usage_count}</span>
              </button>
            {/each}
          </div>
        </section>
      {/if}

      {#if rows.length === 0}
        <p class="msg" data-testid="empty">
          Nothing is tagged yet. Traits appear here once identification has run over some assets —
          they are what AI search matches on, so an empty vocabulary means an unsearchable library.
        </p>
      {:else if byNamespace.length === 0}
        <p class="msg" data-testid="empty">No trait matches “{q}”.</p>
      {:else}
        <div class="namespaces" data-testid="vocab">
          {#each byNamespace as group (group.namespace)}
            <section>
              <h2>{group.namespace}<span class="count num">{group.traits.length}</span></h2>
              <ul>
                {#each group.traits as t (t.id)}
                  {@const tinted = t.color ? `var(${t.color})` : undefined}
                  <li class="row" style="--tint: {tinted ?? 'var(--ls-text-dim)'}">
                    <button class="trait" onclick={() => browse(t.value)} data-testid="trait-row">
                      <i class="dot" aria-hidden="true"></i>
                      <span class="name">{t.value}</span>
                      {#if t.scope}
                        <span class="pill scope">{t.scope}</span>
                      {/if}
                      <span class="stat num">
                        {t.usage_count.toLocaleString()}
                        <span class="split">
                          ({t.manual_count.toLocaleString()} manual · {t.ai_count.toLocaleString()} AI)
                        </span>
                      </span>
                    </button>
                    <div class="ops">
                      <button
                        class="op teach"
                        class:on={t.teach}
                        onclick={() => write(() => setTraitTeach(t.id, !t.teach))}
                        title="Teach the identifier to recognise this trait"
                        data-testid="teach-{t.value}"
                      >
                        {t.teach ? "✦ teaching" : "✦ teach"}
                      </button>
                      <div class="swatches" role="group" aria-label="Chip colour">
                        {#each STYLE_SWATCHES as sw (sw)}
                          <button
                            class="swatch"
                            class:on={t.color === sw}
                            style="--sw: var({sw})"
                            aria-label={sw}
                            onclick={() => write(() => setTraitStyle(t.id, t.color === sw ? null : sw))}
                          ></button>
                        {/each}
                      </div>
                      <button
                        class="op demote"
                        onclick={() => write(() => unpromoteTrait(t.id))}
                        title="Demote to a plain tag — assignments stay"
                        data-testid="demote-{t.value}"
                      >
                        ✕
                      </button>
                    </div>
                  </li>
                {/each}
              </ul>
            </section>
          {/each}
        </div>
        {/if}
      {/if}
    {:else}
      <p class="subtitle">
        Rules that sort messy tags into your clean categories — swap whole vocabularies without
        touching a single tag.
      </p>

      {#if mapError}
        <p class="msg danger">{mapError}</p>
      {:else if !taxRows}
        <p class="msg">Loading…</p>
      {:else}
        {#if mapActionError}
          <p class="msg danger">{mapActionError}</p>
        {/if}

        <section class="card" data-testid="taxonomy-list">
          <h2>Taxonomies</h2>
          <p class="hint">
            One taxonomy is active at a time — the rules that follow map your tags onto its
            category tree. Creating taxonomies is not part of this build; the ones that exist
            can be switched and deleted.
          </p>
          {#if taxRows.length === 0}
            <p class="msg" data-testid="mapping-empty-taxonomies">No taxonomies yet — nothing to map.</p>
          {:else}
            <ul class="taxonomies">
              {#each taxRows as t (t.id)}
                <li class="taxonomy-row" class:on={t.is_active} data-testid="taxonomy-row-{t.name}">
                  <button
                    class="switch"
                    onclick={() => switchActive(t.id)}
                    disabled={t.is_active}
                    title={t.is_active ? `${t.name} is the active taxonomy` : `Make ${t.name} the active taxonomy`}
                    data-testid="taxonomy-switch-{t.name}"
                  >
                    <span class="mark" aria-hidden="true">{t.is_active ? "●" : "○"}</span>
                    <span class="name">{t.name}</span>
                  </button>
                  <span class="version num">v{t.version}</span>
                  {#if t.is_active}
                    <span class="pill active-pill">active</span>
                  {/if}
                  <button
                    class="op danger-op"
                    onclick={() => askTaxonomyDelete(t)}
                    aria-label="Delete taxonomy {t.name}"
                    title="Delete {t.name} and everything in it"
                    data-testid="taxonomy-delete-{t.name}"
                  >
                    ✕
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section class="card" data-testid="mapping-tree">
          <h2>Category tree</h2>
          <p class="hint">
            Drawn from the rules below — a category appears once a rule targets it (rule
            authoring is not in this build). Adding a category creates the row in the taxonomy;
            renaming rewrites the paths of its whole subtree; deleting removes the subtree and
            its mapped assignments, never the raw tags on your assets.
          </p>
          {#if !activeTaxonomy}
            <p class="msg">Pick an active taxonomy to manage its categories.</p>
          {:else}
            {#if catTree.length === 0}
              <p class="msg" data-testid="mapping-empty-categories">
                No categories yet — add a root category to start.
              </p>
            {/if}
            {#if edit?.mode === "add-root"}
              <div class="inline-form root-form">
                <input
                  bind:value={draft}
                  aria-label="Root category name"
                  placeholder="Root category name"
                  data-testid="category-add-root-input"
                  onkeydown={(e) => e.key === "Enter" && addRoot()}
                />
                <button class="gold" onclick={addRoot} data-testid="category-add-root-go">＋ Add</button>
                <button class="op" onclick={() => (edit = null)}>Cancel</button>
              </div>
            {:else}
              <button class="ghost" onclick={startAddRoot} data-testid="category-add-root">
                ＋ Add root category
              </button>
            {/if}
            {#if treeRows.length > 0}
              <ul class="tree">
                {#each treeRows as { node, depth } (node.id)}
                  <li class="tree-row" style="--depth: {depth}" data-testid="category-row-{node.id}">
                    <div class="row-main">
                      {#if node.children.length > 0}
                        <button
                          class="caret"
                          onclick={() => toggleCat(node.id)}
                          aria-label="Toggle children of {node.label}"
                          data-testid="category-caret-{node.id}"
                        >
                          {collapsed.has(node.id) ? "▸" : "▾"}
                        </button>
                      {:else}
                        <span class="caret placeholder" aria-hidden="true"></span>
                      {/if}
                      {#if edit?.mode === "rename" && edit.id === node.id}
                        <div class="inline-form">
                          <input
                            bind:value={draft}
                            aria-label="New name for {node.label}"
                            placeholder={node.label}
                            data-testid="category-rename-input-{node.id}"
                            onkeydown={(e) => e.key === "Enter" && submitRename()}
                          />
                          <button class="gold" onclick={submitRename} data-testid="category-rename-go-{node.id}">
                            Save
                          </button>
                          <button class="op" onclick={() => (edit = null)}>Cancel</button>
                        </div>
                      {:else}
                        <span class="label">{node.label}</span>
                        <span class="path num">{node.path}</span>
                        <div class="ops">
                          <button
                            class="op"
                            onclick={() => startAddChild(node)}
                            title="Add a child category under {node.label}"
                            aria-label="Add child under {node.label}"
                            data-testid="category-add-child-{node.id}"
                          >
                            ＋
                          </button>
                          <button
                            class="op"
                            onclick={() => startRename(node)}
                            title="Rename {node.label}"
                            aria-label="Rename {node.label}"
                            data-testid="category-rename-{node.id}"
                          >
                            ✎
                          </button>
                          <button
                            class="op danger-op"
                            onclick={() => askCategoryDelete(node)}
                            title="Delete {node.path} and its subtree"
                            aria-label="Delete {node.path}"
                            data-testid="category-delete-{node.id}"
                          >
                            ✕
                          </button>
                        </div>
                      {/if}
                    </div>
                    {#if edit?.mode === "add-child" && edit.id === node.id}
                      <div class="inline-form child-form">
                        <input
                          bind:value={draft}
                          aria-label="Child category name"
                          placeholder="Child category name"
                          data-testid="category-add-child-input-{node.id}"
                          onkeydown={(e) => e.key === "Enter" && addChild()}
                        />
                        <button class="gold" onclick={addChild} data-testid="category-add-child-go-{node.id}">
                          ＋ Add
                        </button>
                        <button class="op" onclick={() => (edit = null)}>Cancel</button>
                      </div>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        </section>

        <section class="card" data-testid="mapping-rules">
          <h2>Rules</h2>
          <p class="hint">
            Read-only in this build — rule authoring is not part of the slice. Rules run in
            priority order (lower wins); the first match claims the asset.
          </p>
          {#if !activeTaxonomy}
            <p class="msg">Pick an active taxonomy to see its rules.</p>
          {:else if (rules ?? []).length === 0}
            <p class="msg" data-testid="mapping-empty-rules">No rules in this taxonomy yet.</p>
          {:else}
            <ul class="rules">
              {#each rules as r (r.id)}
                <li class="rule-row" class:off={!r.enabled} data-testid="rule-row-{r.id}">
                  <span class="pill match">{r.match_type}</span>
                  <span class="pattern num">{r.pattern}</span>
                  <span class="arrow" aria-hidden="true">→</span>
                  <span class="target">{r.category_path}</span>
                  <span class="prio num">p{r.priority}</span>
                  <span class="pill state">{r.enabled ? "on" : "off"}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </section>

        <section class="card apply-bar">
          <div class="apply-copy">
            <p class="hint">
              Applies the active taxonomy's rules to the whole library: each asset goes to the
              first category that matches, pinned (manual) assignments survive, and the taxonomy
              version bumps. Raw tags are never modified.
            </p>
            {#if applied}
              <p class="msg ok" data-testid="mapping-apply-result">{applied}</p>
            {/if}
          </div>
          <button
            class="gold"
            onclick={applyActive}
            disabled={!activeTaxonomy || applying}
            data-testid="mapping-apply"
          >
            {applying ? "Applying…" : "⚙ Apply"}
          </button>
        </section>
      {/if}

      {#if confirm}
        <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
        <div class="scrim" onclick={() => (confirm = null)} data-testid="mapping-confirm">
          <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
          <div
            class="sheet"
            onclick={(e) => e.stopPropagation()}
            role="alertdialog"
            tabindex="-1"
            aria-modal="true"
            aria-labelledby="mapping-confirm-title"
          >
            <h3 id="mapping-confirm-title">
              Delete {confirm.kind} “{confirm.name}”?
            </h3>
            <p>
              {#if confirm.kind === "taxonomy"}
                Its categories, rules and mapped assignments are removed with it. Your assets'
                raw tags are untouched.
              {:else}
                Its subtree, rules and mapped assignments are removed with it. Your assets' raw
                tags are untouched.
              {/if}
            </p>
            <div class="confirm-ops">
              <button onclick={() => (confirm = null)} data-testid="mapping-confirm-cancel">Cancel</button>
              <button class="danger" onclick={onConfirmGo} data-testid="mapping-confirm-go">
                {confirm.kind === "taxonomy" ? "Delete taxonomy" : "Delete category"}
              </button>
            </div>
          </div>
        </div>
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
    width: 240px;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 var(--sp-5) var(--sp-5);
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
    max-width: 900px;
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
  .fields {
    display: flex;
    gap: var(--sp-2);
  }
  .fields input {
    flex: 1;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
  }
  .fields input:focus {
    outline: none;
    border-color: var(--ls-gold);
  }
  .gold {
    height: var(--control-h);
    padding: 0 var(--sp-3);
    background: none;
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-md);
    color: var(--ls-gold-bright);
    font: inherit;
    cursor: pointer;
    white-space: nowrap;
  }
  .gold:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ls-gold) 15%, transparent);
  }
  .gold:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 3px var(--sp-2);
    background: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
    color: var(--ls-text-muted);
    font: inherit;
    font-size: var(--fs-xs);
    cursor: pointer;
  }
  .chip:hover {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .count {
    color: var(--ls-text-dim);
  }
  .namespaces {
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
  }
  .namespaces h2 {
    display: flex;
    align-items: baseline;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-2);
    font-size: var(--fs-lg);
  }
  .namespaces h2 .count {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0 var(--sp-2);
    border-bottom: 1px solid color-mix(in srgb, var(--ls-border) 50%, transparent);
    background: var(--ls-surface);
  }
  .row:last-child {
    border-bottom: none;
  }
  .trait {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0;
    background: none;
    border: none;
    color: var(--ls-text);
    font: inherit;
    cursor: pointer;
    text-align: left;
    height: var(--row-h);
  }
  .trait:hover .name {
    color: var(--ls-gold);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--tint, var(--ls-text-dim));
    flex: none;
    transform: rotate(45deg);
  }
  .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill {
    padding: 1px var(--sp-2);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }
  .pill.scope {
    color: var(--ls-arcane-bright);
    border-color: color-mix(in srgb, var(--ls-arcane) 45%, transparent);
  }
  .stat {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    white-space: nowrap;
  }
  .split {
    color: var(--ls-text-muted);
    opacity: 0.7;
  }
  .ops {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    flex: none;
  }
  .op {
    background: none;
    border: 1px solid var(--ls-border);
    border-radius: var(--r-sm);
    color: var(--ls-text-dim);
    font: inherit;
    font-size: var(--fs-xs);
    cursor: pointer;
    padding: 2px var(--sp-2);
  }
  .op:hover {
    border-color: var(--ls-text-muted);
    color: var(--ls-text);
  }
  .op.teach.on {
    color: var(--ls-arcane-bright);
    border-color: var(--ls-arcane);
    background: color-mix(in srgb, var(--ls-arcane) 14%, transparent);
  }
  .op.demote {
    color: var(--ls-danger);
    border-color: transparent;
  }
  .op.demote:hover {
    border-color: var(--ls-danger);
  }
  .swatches {
    display: flex;
    gap: 3px;
    margin-left: var(--sp-1);
  }
  .swatch {
    width: 14px;
    height: 14px;
    border-radius: 3px;
    border: 1px solid var(--ls-border-strong);
    background: var(--sw);
    cursor: pointer;
    padding: 0;
  }
  .swatch:hover {
    transform: scale(1.15);
  }
  .swatch.on {
    outline: 2px solid var(--ls-gold);
    outline-offset: 1px;
  }
  .msg {
    color: var(--ls-text-dim);
    margin: 0;
  }
  .msg.danger {
    color: var(--ls-danger);
  }

  /* ---- Tab switcher (UI-SPEC §9) ---- */
  .tabs {
    display: flex;
    gap: 2px;
    padding: 2px;
    margin-left: var(--sp-4);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .tab {
    padding: 3px var(--sp-3);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
    white-space: nowrap;
    cursor: pointer;
  }
  .tab:hover:not(.on) {
    color: var(--ls-text);
  }
  .tab.on {
    background: var(--ls-overlay);
    color: var(--ls-text);
  }

  /* ---- Mapping tab (§9.2) ---- */
  .subtitle {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
  }
  .taxonomies,
  .tree,
  .rules {
    list-style: none;
    margin: 0;
    padding: 0;
    /* The component's global `ul` rule adds a border+overflow:hidden box; these lists are made
       of individually-bordered rows, so drop the inherited box. */
    border: none;
    border-radius: 0;
    overflow: visible;
  }
  .taxonomies,
  .rules {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .taxonomy-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
  }
  .taxonomy-row.on {
    border-color: var(--ls-gold);
  }
  .switch {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 0;
    background: none;
    border: none;
    color: var(--ls-text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .switch:disabled {
    cursor: default;
  }
  .switch:not(:disabled):hover .name {
    color: var(--ls-gold);
  }
  .mark {
    color: var(--ls-gold);
    flex: none;
  }
  .taxonomy-row:not(.on) .mark {
    color: var(--ls-text-dim);
  }
  .version {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    white-space: nowrap;
  }
  .active-pill {
    color: var(--ls-gold);
    border-color: color-mix(in srgb, var(--ls-gold) 45%, transparent);
  }
  .op.danger-op {
    color: var(--ls-danger);
    border-color: transparent;
  }
  .op.danger-op:hover {
    border-color: var(--ls-danger);
  }
  .ghost {
    height: var(--control-h);
    padding: 0 var(--sp-3);
    background: none;
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text-muted);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .ghost:hover {
    border-color: var(--ls-gold);
    color: var(--ls-text);
  }
  .tree {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: var(--sp-2);
  }
  .tree-row {
    padding-left: calc(var(--depth) * 14px);
  }
  .row-main {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    min-height: var(--row-h);
  }
  .caret {
    width: 18px;
    flex: none;
    background: none;
    border: none;
    color: var(--ls-text-dim);
    font: inherit;
    cursor: pointer;
  }
  .caret.placeholder {
    display: inline-block;
  }
  .label {
    font-size: var(--fs-sm);
    color: var(--ls-text);
    white-space: nowrap;
  }
  .path {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .inline-form {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    flex: 1;
    min-width: 0;
  }
  .inline-form input {
    flex: 1;
    min-width: 0;
    height: var(--control-h);
    padding: 0 var(--sp-2);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-md);
    color: var(--ls-text);
    font: inherit;
  }
  .inline-form input:focus {
    outline: none;
    border-color: var(--ls-gold);
  }
  .root-form {
    margin: var(--sp-2) 0 0;
  }
  .child-form {
    padding: 0 0 var(--sp-1) var(--sp-5);
  }
  .rule-row {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
  }
  .rule-row.off {
    opacity: 0.55;
  }
  .match {
    color: var(--ls-arcane-bright);
    border-color: color-mix(in srgb, var(--ls-arcane) 45%, transparent);
  }
  .pattern {
    font-family: var(--ff-mono);
    color: var(--ls-text);
    white-space: nowrap;
  }
  .arrow {
    color: var(--ls-text-dim);
    flex: none;
  }
  .target {
    color: var(--ls-text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .prio {
    margin-left: auto;
    color: var(--ls-text-dim);
    white-space: nowrap;
  }
  .state {
    color: var(--ls-text-muted);
  }
  .rule-row:not(.off) .state {
    color: var(--ls-success);
    border-color: color-mix(in srgb, var(--ls-success) 45%, transparent);
  }
  .apply-bar {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .apply-copy {
    flex: 1;
    min-width: 0;
  }
  .apply-bar .hint {
    margin: 0;
  }
  .msg.ok {
    color: var(--ls-success);
  }
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.6);
    display: grid;
    place-items: center;
    z-index: 100;
  }
  .sheet {
    width: min(460px, 92vw);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    padding: var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
  }
  .sheet h3 {
    font-size: var(--fs-lg);
  }
  .sheet p {
    margin: 0;
    color: var(--ls-text-dim);
    font-size: var(--fs-sm);
  }
  .confirm-ops {
    display: flex;
    justify-content: flex-end;
    gap: var(--sp-2);
  }
  .confirm-ops button {
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-md);
    border: 1px solid var(--ls-border-strong);
    background: var(--ls-surface);
    color: var(--ls-text);
    font: inherit;
    cursor: pointer;
  }
  .confirm-ops .danger {
    border-color: var(--ls-danger);
    color: var(--ls-danger);
  }
  .confirm-ops .danger:hover {
    background: var(--ls-danger);
    color: var(--ls-bg);
  }
</style>
