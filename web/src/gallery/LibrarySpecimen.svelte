<script lang="ts">
  import { untrack } from "svelte";
  import Shell from "../app/Shell.svelte";
  import type { ShellData } from "../app/shell";
  import type { AssetIntent } from "../features/asset/actions";
  import type { AssetDetail } from "../features/asset/model";
  import Inspector from "../features/inspector/Inspector.svelte";
  import { DEFAULT_VIEW, type LibraryView } from "../features/library/view";
  import { ASSETS, COLLECTIONS, PROJECT, SIZE_THRESHOLDS, TRAITS, shellData } from "../fixtures/library";
  import LibraryScreen from "../screens/LibraryScreen.svelte";
  import { Selection } from "../state/selection.svelte";
  import { Toast } from "../ui";

  /*
   * The Library in the shell, on the prototype's data — what the gate measures against the
   * contract at 1440 × 900 (`gallery.html#library`: state `library`; `#library-inspector`: state
   * `library+inspector`, with the first asset selected).
   *
   * This is the only owner of data on the page: it stands in for the resource cache and the
   * mutation layer of plan P6b. Intents change its copy of the fixture, so favourite, stage, trash
   * and the selection can be tried by hand; an intent it has no stand-in for is shown as a toast.
   */
  let { select = null }: { select?: number | null } = $props();

  const selection = new Selection();
  untrack(() => select !== null && selection.moveTo(select));

  let assets = $state<AssetDetail[]>(ASSETS.map((a) => ({ ...a })));
  let shell = $state<ShellData>(shellData());
  let view = $state<LibraryView>({ ...DEFAULT_VIEW });
  let nudgeDismissed = $state(false);
  let said = $state<string | null>(null);

  const say = (message: string) => (said = message);

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

  const tris = (a: AssetDetail) => a.stats.tris ?? 0;
  const page = $derived.by(() => {
    const list = assets.filter((a) => !view.favoritesOnly || a.favorite);
    if (view.sort === "name") return [...list].sort((a, b) => a.name.localeCompare(b.name));
    if (view.sort === "sizeDesc") return [...list].sort((a, b) => b.size_bytes - a.size_bytes);
    if (view.sort === "sizeAsc") return [...list].sort((a, b) => a.size_bytes - b.size_bytes);
    if (view.sort === "tris") return [...list].sort((a, b) => tris(b) - tris(a));
    return list;
  });

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
    unidentified={shell.badges.identify ?? 0}
    {nudgeDismissed}
    {view}
    favoriteCount={assets.filter((a) => a.favorite).length}
    {selection}
    project={PROJECT}
    traits={TRAITS}
    thresholds={SIZE_THRESHOLDS}
    onintent={carryOut}
    onview={(patch) => (view = { ...view, ...patch })}
    onfilters={() => say("The ⚑ Filters popover is its own task")}
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
