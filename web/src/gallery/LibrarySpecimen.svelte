<script lang="ts">
  import { onMount, untrack } from "svelte";
  import Shell from "../app/Shell.svelte";
  import { CATEGORIES, type Category, type ShellData } from "../app/shell";
  import type { AssetIntent } from "../features/asset/actions";
  import type { AssetDetail, MediaType } from "../features/asset/model";
  import { userStatus } from "../features/asset/present";
  import type { Constraints, ShellAxis } from "../features/filters/active";
  import { NO_FILTERS, passes, traitCounts, type LibraryFilters } from "../features/filters/filters";
  import Inspector from "../features/inspector/Inspector.svelte";
  import { DEFAULT_VIEW, type LibraryView } from "../features/library/view";
  import { ASSETS, COLLECTIONS, PROJECT, SIZE_THRESHOLDS, TRAITS, shellData } from "../fixtures/library";
  import LibraryScreen from "../screens/LibraryScreen.svelte";
  import { Selection } from "../state/selection.svelte";
  import { Toast } from "../ui";

  /*
   * The Library in the shell, on the prototype's data — what the gate measures against the
   * contract at 1440 × 900. Each `gallery.html#…` page is this component started in one contract
   * state (gallery/main.ts lists them): `#library`, `#library-inspector` (the first asset
   * selected), `#library-filters` (⚑ open with rock ∧ ≤ 1k ∧ M chosen).
   *
   * This is the only owner of data on the page: it stands in for the resource cache, the library
   * query and the mutation layer of plan P6b. Intents change its copy of the fixture, and the
   * shell's facets, the search box, ⚑ and ★ all narrow the page it hands the screen — so every
   * control can be tried by hand; an intent it has no stand-in for is shown as a toast.
   */
  let {
    select = null,
    filters: startFilters = NO_FILTERS,
    open = null,
  }: {
    /** The asset the inspector describes when the page loads. */
    select?: number | null;
    /** The ⚑ filter in effect when the page loads. */
    filters?: LibraryFilters;
    /** An overlay to open once the page is up, the way a person would. */
    open?: "filters" | null;
  } = $props();

  const selection = new Selection();
  untrack(() => select !== null && selection.moveTo(select));

  let assets = $state<AssetDetail[]>(ASSETS.map((a) => ({ ...a })));
  let shell = $state<ShellData>(shellData());
  let view = $state<LibraryView>({ ...DEFAULT_VIEW });
  let filters = $state<LibraryFilters>(untrack(() => startFilters));
  let nudgeDismissed = $state(false);
  let said = $state<string | null>(null);

  const say = (message: string) => (said = message);

  onMount(() => {
    // Opened by pressing the real button, so the popover is anchored and focused as in use.
    if (open === "filters") document.querySelector<HTMLElement>('[data-anchor="library.filters"]')?.click();
  });

  // The inspector describes the lead — looked up in the data every time, never kept.
  const inspected = $derived(assets.find((a) => a.id === selection.lead) ?? null);

  function setCollection(id: number, on: boolean) {
    const lead = selection.lead;
    assets = assets.map((a) =>
      a.id === lead ? { ...a, collection_ids: on ? [...a.collection_ids, id] : a.collection_ids.filter((c) => c !== id) } : a,
    );
  }

  function addTag(name: string) {
    const lead = selection.lead;
    assets = assets.map((a) =>
      a.id === lead ? { ...a, tags: [...a.tags, { namespace: null, value: name, confidence: null, source: "manual" }] } : a,
    );
  }

  function openAsset(id: number) {
    if (page.some((a) => a.id === id)) selection.moveTo(id);
    else say("That asset is hidden by the current filter");
  }

  // ---- the library query's stand-in: what the shell, ⚑ and ★ leave, in the chosen order ----

  const MEDIA: Record<Exclude<Category, "all">, MediaType[]> = {
    model: ["model_3d"],
    material: ["material"],
    sound: ["audio"],
    image: ["image", "texture", "sprite"],
  };

  function inScope(a: AssetDetail): boolean {
    if (shell.status.value !== "all" && userStatus(a.status) !== shell.status.value) return false;
    if (shell.libraries.value !== "all" && String(a.library.id) !== shell.libraries.value) return false;
    if (shell.category !== "all" && !MEDIA[shell.category].includes(a.media_type)) return false;
    const query = shell.query.trim().toLowerCase();
    if (!query) return true;
    return `${a.name} ${a.package?.name ?? ""} ${a.tags.map((t) => t.value).join(" ")}`.toLowerCase().includes(query);
  }

  const tris = (a: AssetDetail) => a.stats.tris ?? 0;
  const page = $derived.by(() => {
    const list = assets.filter((a) => inScope(a) && (!view.favoritesOnly || a.favorite) && passes(a, filters, SIZE_THRESHOLDS));
    if (view.sort === "name") return [...list].sort((a, b) => a.name.localeCompare(b.name));
    if (view.sort === "sizeDesc") return [...list].sort((a, b) => b.size_bytes - a.size_bytes);
    if (view.sort === "sizeAsc") return [...list].sort((a, b) => a.size_bytes - b.size_bytes);
    if (view.sort === "tris") return [...list].sort((a, b) => tris(b) - tris(a));
    return list;
  });

  const library = $derived(shell.libraries.options.find((o) => o.id === shell.libraries.value && o.id !== "all"));
  const category = $derived(CATEGORIES.find((c) => c.value === shell.category && c.value !== "all"));

  /** What the shell constrains the result by, for the active-filter bar. */
  const constraints = $derived<Constraints>({
    query: shell.query,
    category: category ? { id: category.value as Exclude<Category, "all">, label: category.label } : undefined,
    status: shell.status.value === "all" ? undefined : shell.status.value,
    library: library ? { id: library.id, name: library.label } : undefined,
  });

  function unconstrain(axis: ShellAxis) {
    if (axis === "query") shell.query = "";
    else if (axis === "type") shell.category = "all";
    else if (axis === "status") shell.status.value = "all";
    else if (axis === "library") shell.libraries.value = "all";
  }

  /** The reset the sidebar's Library row performs (UI-SPEC §0.2), with a name and a place. */
  function clearFilters() {
    shell.query = "";
    shell.category = "all";
    shell.status.value = "all";
    shell.libraries.value = "all";
    filters = NO_FILTERS;
    view = { ...view, favoritesOnly: false };
  }

  function carryOut(intent: AssetIntent) {
    const hit = (a: AssetDetail) => intent.ids.includes(a.id);
    if (intent.action === "favorite") {
      assets = assets.map((a) => (hit(a) ? { ...a, favorite: intent.on ?? !a.favorite } : a));
      say(intent.on ? "★ Added to Favorites" : "Removed from Favorites");
    } else if (intent.action === "stage") {
      assets = assets.map((a) => (hit(a) ? { ...a, staged: intent.on ?? !a.staged } : a));
    } else if (intent.action === "trash") {
      assets = assets.filter((a) => !hit(a));
      say(`Moved ${intent.ids.length} to Trash`);
    } else say(`${intent.action} · ${intent.ids.join(", ")}`);
  }
</script>

{#snippet panel()}
  {#if inspected}
    <Inspector
      asset={inspected}
      collections={COLLECTIONS}
      traits={TRAITS}
      project={PROJECT}
      anchored
      onintent={carryOut}
      onclose={() => (selection.lead = null)}
      oncollection={setCollection}
      onaddtag={addTag}
      onopenasset={openAsset}
      onsearchtag={(tag) => (shell.query = tag)}
      onpack={(pack) => say(`Pack stats · ${pack.name}`)}
    />
  {/if}
{/snippet}

<Shell
  data={shell}
  inspector={inspected ? panel : undefined}
  onnavigate={(id) => (shell.route = id)}
  onmode={(m) => (shell.mode = m)}
  onquery={(q) => (shell.query = q)}
  oncategory={(c) => (shell.category = c)}
  onstatus={(s) => (shell.status.value = s)}
  onlibrary={(l) => (shell.libraries.value = l)}
>
  <LibraryScreen
    assets={page}
    total={assets.length}
    scope={library?.label ?? "all libraries"}
    unidentified={shell.badges.identify ?? 0}
    {nudgeDismissed}
    {view}
    {filters}
    traitOptions={traitCounts(assets)}
    {constraints}
    favoriteCount={assets.filter((a) => a.favorite).length}
    {selection}
    project={PROJECT}
    traits={TRAITS}
    thresholds={SIZE_THRESHOLDS}
    onintent={carryOut}
    onview={(patch) => (view = { ...view, ...patch })}
    onfilterschange={(next) => (filters = next)}
    onunconstrain={unconstrain}
    onclearfilters={clearFilters}
    onidentify={() => (shell.route = "identify")}
    ondismissnudge={() => (nudgeDismissed = true)}
    onpack={(pack) => say(`Pack stats · ${pack.name}`)}
    onsearchtag={(tag) => (shell.query = tag)}
  />
</Shell>

{#if said}
  <div class="said">
    <Toast tone="gold" message={said} ondismiss={() => (said = null)} />
  </div>
{/if}

<style>
  /* The specimen's own feedback, out of the frame's way. */
  .said {
    position: fixed;
    right: var(--space-16);
    bottom: var(--space-16);
    z-index: var(--z-toast);
  }
</style>
