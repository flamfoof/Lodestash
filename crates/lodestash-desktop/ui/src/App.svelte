<script lang="ts">
  import { setContext } from "svelte";
  import TopBar from "./lib/TopBar.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import Screen from "./lib/Screen.svelte";
  import Inspector from "./lib/Inspector.svelte";
  import LibraryScreen from "./lib/LibraryScreen.svelte";
  import StagingTray from "./lib/StagingTray.svelte";
import JobTray from "./lib/JobTray.svelte";
  import { SCREENS } from "./lib/routes";
  import { view } from "./lib/nav.svelte";
  import ConfirmDelete from "./lib/ConfirmDelete.svelte";
  import Palette from "./lib/Palette.svelte";
  import StatsScreen from "./lib/StatsScreen.svelte";
  import DuplicatesScreen from "./lib/DuplicatesScreen.svelte";
  import PackagesScreen from "./lib/PackagesScreen.svelte";
  import TraitsScreen from "./lib/TraitsScreen.svelte";
  import SettingsScreen from "./lib/SettingsScreen.svelte";
  import IdentifyScreen from "./lib/IdentifyScreen.svelte";
  import CollectionsScreen from "./lib/CollectionsScreen.svelte";
  import UnityAssetsScreen from "./lib/UnityAssetsScreen.svelte";
  import DropZone from "./lib/DropZone.svelte";
  import {
    libraryStatus,
    onChanged,
    reloadLibrary,
    assetDetail,
    identifyAsset,
    setFavorite,
    setTrashed,
    engineImport,
    addSourceDialog,
    pickPackageFolder,
    removeFromLibrary,
    deleteFromDisk,
    getWebPrefs,
    setWebPrefs,
    getSettings,
    staged,
    stage,
    unstage,
    clearStaged,
    type AssetDetail,
    type LibraryStatus,
  } from "./lib/api";
  import { refreshTraitColors } from "./lib/traits.svelte";
  import Onboarding from "./lib/Onboarding.svelte";
  import SourceChooser from "./lib/SourceChooser.svelte";
  import ImportDialog from "./lib/ImportDialog.svelte";
  import { push, dismiss, toasts } from "./lib/toasts.svelte";

  let status = $state<LibraryStatus | null>(null);
  let reloading = $state(false);

  // List-row height is token-driven: the frame sets `data-density`, tokens.css maps it to
  // `--row-h` / `--control-h` (32 / 36 / 40 per DESIGN-GUIDELINES §4) — one source of truth.

  // Re-ask the sidebar's facet counts whenever the route changes — navigating is the cheap
  // catch-all. Catalog mutations no longer poll here: the writer pushes `ls:changed` events
  // (pump events, B2b) and `onCatalogChanged` below refreshes status + the grid page.
  $effect(() => {
    view.route;
    let stale = false;
    libraryStatus().then((s) => {
      if (!stale) status = s;
    });
    return () => {
      stale = true;
    };
  });

  // Pump events (B2b): the writer's change notification. "catalog" = the library changed —
  // refresh status and bump the page version so the current grid/list re-reads.
  $effect(() => {
    const off = onChanged("catalog", async () => {
      try {
        status = await libraryStatus();
      } catch {
        /* the next event or navigation re-asks */
      }
      view.dataVersion += 1;
    });
    return off;
  });

  libraryStatus()
    .then((s) => (status = s))
    .catch(() => {});
  // Chip colours for styled traits (B8a) — loaded once, refreshed after every vocabulary write.
  refreshTraitColors();

  // First-run onboarding (FEATURES §8): over the live library, unless the user said skip it.
  let onboardingOpen = $state(false);
  getSettings()
    .then((s) => {
      confirmBeforeTrash = s.confirm_before_trash;
      if (!s.skip_onboarding) onboardingOpen = true;
    })
    .catch(() => {});

  // Confirm-before-trash (UI-SPEC §11): when the setting is on, the first trash click arms a
  // 3s window that labels the next click as the actual trash. Honest two-step, no modal.
  let confirmBeforeTrash = $state(false);
  let trashArmed = $state(false);
  let trashTimer: ReturnType<typeof setTimeout> | undefined;
  function confirmTrash(action: () => void) {
    if (!confirmBeforeTrash) {
      action();
      return;
    }
    if (trashArmed) {
      trashArmed = false;
      clearTimeout(trashTimer);
      action();
      return;
    }
    trashArmed = true;
    clearTimeout(trashTimer);
    trashTimer = setTimeout(() => (trashArmed = false), 3000);
    push("Click again to confirm — moving to Trash", "warn");
  }

  // Persisted view prefs (FEATURES §10): hydrate once at startup, then write the whole object
  // back on any change. The save effect is gated on `hydrated` — without the gate it would fire
  // on mount with *defaults* and clobber the stored prefs before hydration resolves, and every
  // launch would write config twice.
  let hydrated = $state(false);
  $effect(() => {
    getWebPrefs()
      .then((p) => {
        view.hydrate(p);
        hydrated = true;
      })
      .catch(() => {
        // Load failed — still allow saves; nothing was clobbered because none were sent.
        hydrated = true;
      });
  });
  $effect(() => {
    if (!hydrated) return;
    setWebPrefs(view.snapshot()).catch(() => {});
  });

  async function reload() {
    reloading = true;
    try {
      status = await reloadLibrary();
    } finally {
      reloading = false;
    }
  }

  let screen = $derived(SCREENS[view.route]);
  // Favorites shares the library body; it is a *filter over the library*, not its own data.
  // Routing it here without passing the flag renders the whole library under a "Favorites"
  // heading, which reads as though everything is starred.
  let onLibrary = $derived(
    view.route === "library" || view.route === "favorites" || view.route === "trash",
  );
  let favoritesOnly = $derived(view.route === "favorites");
  let trashedOnly = $derived(view.route === "trash");

  // The Inspector's actions need the selected asset's own flags, and the delete dialog needs its
  // absolute path — the user is entitled to see exactly which file is about to stop existing.
  let selectedDetail = $state<AssetDetail | null>(null);
  $effect(() => {
    const id = view.selected;
    view.dataVersion;
    if (id == null) {
      selectedDetail = null;
      return;
    }
    let stale = false;
    assetDetail(id).then((d) => {
      if (!stale) selectedDetail = d;
    });
    return () => {
      stale = true;
    };
  });

  let confirming = $state(false);
  let actionError = $state<string | null>(null);

  /**
   * Run a write, surface any failure, and make every screen re-ask its question. On success,
   * `ok` (when given) turns the result into a confirmation toast (B10c) — failures stay on the
   * action-error bar, never a toast.
   */
  async function write(run: () => Promise<unknown>, ok?: (changed: number) => string | null) {
    actionError = null;
    try {
      const res = (await run()) as { changed?: number } | null | undefined;
      view.dataVersion += 1;
      status = await libraryStatus();
      // A write may have styled/demoted a trait — chips across the app should follow.
      refreshTraitColors();
      if (ok) {
        const changed = typeof res?.changed === "number" ? res.changed : 1;
        const msg = ok(changed);
        if (msg) push(msg);
      }
    } catch (e) {
      actionError = String(e);
    }
  }

  // ---- Staging tray (I6) -------------------------------------------------
  // The tray's ids live here, loaded from the backend once and re-read after every mutation.
  // Cards reach in through Svelte context (set below) rather than a prop chain that would have to
  // thread LibraryScreen → Card; the tray itself gets ids + handlers as plain props.
  const STAGING_CTX = "ls:staging";
  let stagedIds = $state<number[]>([]);
  staged()
    .then((ids) => (stagedIds = ids))
    .catch(() => {});

  /** Toggle one asset in/out of the tray, then re-read the authoritative list. */
  async function toggleStage(id: number) {
    const on = stagedIds.includes(id);
    try {
      if (on) await unstage([id]);
      else await stage([id]);
      stagedIds = await staged();
    } catch (e) {
      actionError = String(e);
    }
  }

  async function unstageOne(id: number) {
    try {
      await unstage([id]);
      stagedIds = await staged();
    } catch (e) {
      actionError = String(e);
    }
  }

  async function clearTray() {
    try {
      await clearStaged();
      stagedIds = await staged();
    } catch (e) {
      actionError = String(e);
    }
  }

  setContext<{ isStaged: (id: number) => boolean; toggleStage: (id: number) => void }>(STAGING_CTX, {
    // Reads `stagedIds` at call time; a `$state` read during a card's render is reactive, so the
    // card's ⧉ reflects tray membership without any extra prop wiring.
    isStaged: (id) => stagedIds.includes(id),
    toggleStage,
  });

  let paletteOpen = $state(false);

  // "Add a source" chooser (design README §"Add a source chooser"): Watch a folder (the
  // existing native import) or Unity Asset Store packages (the §5.2 import dialog).
  let chooserOpen = $state(false);
  let importRoot = $state<string | null>(null);

  async function watchFolder() {
    chooserOpen = false;
    try {
      const r = await addSourceDialog();
      if (r === null) return;
      view.dataVersion += 1;
      status = await libraryStatus();
    } catch (e) {
      actionError = String(e);
    }
  }

  async function importFromFolder() {
    chooserOpen = false;
    try {
      const p = await pickPackageFolder();
      if (p) importRoot = p;
    } catch (e) {
      actionError = String(e);
    }
  }

  // Re-pick the folder from inside the open import dialog (the parent owns the path).
  async function repickImportFolder() {
    try {
      const p = await pickPackageFolder();
      if (p) importRoot = p;
    } catch (e) {
      actionError = String(e);
    }
  }

  /**
   * Ctrl/Cmd-K opens the palette from anywhere — including from inside the search box, which is
   * the one place a global shortcut usually stops working and the one place people press it.
   */
  function onkeydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      paletteOpen = !paletteOpen;
      e.preventDefault();
      return;
    }
    // F — favourite the selected asset (UI-SPEC §0.4). Never steals from an input.
    if (e.key.toLowerCase() === "f" && !e.ctrlKey && !e.metaKey && !e.altKey) {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (view.selected == null) return;
      write(() => setFavorite([view.selected!], true), () => "Added to Favorites");
    }
  }

  function afterRemoval() {
    // The asset just left this view; keeping it selected would leave the Inspector describing
    // something the grid no longer shows.
    view.selected = null;
    confirming = false;
  }
  /** Context-menu actions from the grid (FEATURES §1.1) — the write fns live here. */
  function onMenuAction(d: { action: string; id: number; favorite: boolean }) {
    const { action, id, favorite } = d;
    if (action === "favorite") {
      write(() => setFavorite([id], !favorite), () => (favorite ? "Removed from Favorites" : "Added to Favorites"));
    } else if (action === "identify") {
      write(() => identifyAsset(id), () => "Flagged for identify — appraising…").then(() => {
        view.dataVersion += 1;
      });
    } else if (action === "trash") {
      confirmTrash(() => write(() => setTrashed([id], true), () => "Moved to Trash"));
    } else if (action === "import") {
      importToProject([id]);
    }
  }

  /** `⇢ Add to project` (FEATURES §7.2): queue an engine import into the open Unity project. */
  async function importToProject(ids: number[]) {
    if (ids.length === 0) return;
    try {
      const r = await engineImport(ids);
      if (r.kind === "AwaitingApproval") {
        push(`Import queued — approve it in the job tray`, "warn");
      } else {
        push(`Already in the project (${r.detail.ids.length} assets)`, "info");
      }
    } catch (e) {
      push(String(e), "err");
    }
  }
</script>

<svelte:window on:keydown={onkeydown} />

<div class="frame" data-density={view.density}>
  <TopBar engineReady={status?.open ?? false} {status} />
  <div class="body">
    <Sidebar {status} onopensource={() => (chooserOpen = true)} />
    <main>
      {#if view.route === "traits"}
        <TraitsScreen />
      {:else if view.route === "unity"}
        <UnityAssetsScreen />
      {:else if view.route === "collections"}
        <CollectionsScreen />
      {:else if view.route === "identify"}
        <IdentifyScreen />
      {:else if view.route === "settings"}
        <SettingsScreen />
      {:else if view.route === "packages"}
        <PackagesScreen />
      {:else if view.route === "stats"}
        <StatsScreen />
      {:else if view.route === "dups"}
        <DuplicatesScreen
          ontrash={(ids) =>
            confirmTrash(() =>
              write(() => setTrashed(ids, true), (n) =>
                n === 1 ? "Moved to Trash" : `Moved ${n} assets to Trash`,
              ),
            )
          }
        />
      {:else if onLibrary}
        <LibraryScreen
          {status}
          {favoritesOnly}
          {trashedOnly}
          title={favoritesOnly ? "Favorites" : trashedOnly ? "Trash" : "Library"}
          onmenuaction={onMenuAction}
          onrestore={(id) =>
            write(() => setTrashed([id], false), () => "Restored from Trash").then(afterRemoval)
          }
        />
      {:else}
        <Screen title={screen.title} milestone={screen.milestone} />
      {/if}
    </main>
    <!-- UI-SPEC §13: the rail is always mounted on Library screens — collapsed it is a 26px edge
         tab, not an absent component, and it owns reopening itself when a selection arrives. -->
    {#if onLibrary}
      <Inspector
        assetId={view.selected}
        favorite={selectedDetail?.favorite ?? false}
        trashed={selectedDetail?.trashed ?? false}
        onidentify={() => {
          if (view.selected == null) return;
          write(() => identifyAsset(view.selected!), () => "Flagged for identify — appraising…").then(
            () => {
              view.dataVersion += 1;
            },
          );
        }}
        onfavorite={(on) =>
          write(() => setFavorite([view.selected!], on), () =>
            on ? "Added to Favorites" : "Removed from Favorites",
          )
        }
        ontrash={() =>
          confirmTrash(() =>
            write(() => setTrashed([view.selected!], true), () => "Moved to Trash").then(afterRemoval),
          )
        }
        onimport={() => view.selected !== null && importToProject([view.selected])}
        onrestore={() =>
          write(() => setTrashed([view.selected!], false), () => "Restored from Trash").then(afterRemoval)
        }
        onremove={() =>
          write(() => removeFromLibrary([view.selected!]), () => "Removed from Library").then(afterRemoval)
        }
        ondelete={() => (confirming = true)}
      />
    {/if}
  </div>

  <StagingTray
    ids={stagedIds}
    onunstage={unstageOne}
    onclear={clearTray}
    onimport={(ids) => importToProject(ids)}
  />
  <JobTray />

  {#if confirming && selectedDetail}
    <ConfirmDelete
      names={[selectedDetail.name]}
      paths={[selectedDetail.abs_path]}
      onconfirm={() =>
        write(() => deleteFromDisk([selectedDetail!.asset_id]), (n) =>
          n === 1 ? "Deleted from disk" : `Deleted ${n} files from disk`,
        ).then(afterRemoval)
      }
      onremove={() =>
        write(() => removeFromLibrary([selectedDetail!.asset_id]), () => "Removed from Library").then(afterRemoval)
      }
      oncancel={() => (confirming = false)}
    />
  {/if}

  <DropZone onindexed={async () => (status = await libraryStatus())} />

  {#if paletteOpen}
    <Palette onclose={() => (paletteOpen = false)} />
  {/if}

  {#if onboardingOpen}
    <Onboarding onclose={() => (onboardingOpen = false)} />
  {/if}

  {#if chooserOpen}
    <SourceChooser
      onwatch={watchFolder}
      onimport={importFromFolder}
      onclose={() => (chooserOpen = false)}
    />
  {/if}

  {#if importRoot}
    <ImportDialog root={importRoot} onclose={() => (importRoot = null)} onrepick={repickImportFolder} />
  {/if}

  {#if toasts.length > 0}
    <div class="toasts" data-testid="toast-host" aria-live="polite">
      {#each toasts as t (t.id)}
        <div class="toast {t.kind}" data-testid="toast" role="status">
          <span class="msg">{t.msg}</span>
          <button
            class="dismiss"
            data-testid="toast-dismiss"
            aria-label="Dismiss notification"
            onclick={() => dismiss(t.id)}
          >✕</button>
        </div>
      {/each}
    </div>
  {/if}

  {#if actionError}
    <p class="action-error" data-testid="action-error">{actionError}</p>
  {/if}

  <footer class="statusbar">
    {#if status?.open}
      <span class="num">{status.total_assets.toLocaleString()} assets</span>
      <span>·</span>
      <span class="num">{status.with_preview.toLocaleString()} vector previews</span>
      <span>·</span>
      <span>{status.sources.length} source{status.sources.length === 1 ? "" : "s"}</span>
      <button class="reload" onclick={reload} disabled={reloading}>
        {reloading ? "reloading…" : "reload"}
      </button>
      <span class="right path" title={status.catalog_path ?? ""}>{status.catalog_path}</span>
    {:else}
      <span>no catalog — shell scaffold</span>
    {/if}
  </footer>
</div>

<style>
  .frame {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  main {
    flex: 1;
    min-width: 0;
    background: var(--ls-bg);
  }
  /* Toasts (I5/B10c): bottom-right, floating above the 24px statusbar (DESIGN-GUIDELINES §7 —
     `--ls-overlay`, 1px accent left-border by type: ok → success, err → danger, warn → warning,
     info → info). */
  .toasts {
    position: fixed;
    right: var(--sp-4);
    bottom: calc(24px + var(--sp-4));
    z-index: 80;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--sp-2);
    pointer-events: none;
  }
  .toast {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    max-width: 340px;
    padding: var(--sp-2) var(--sp-3);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border);
    border-left: 1px solid var(--accent, var(--ls-success));
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
    color: var(--ls-text);
    pointer-events: auto;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
  }
  .toast.ok {
    --accent: var(--ls-success);
  }
  .toast.err {
    --accent: var(--ls-danger);
  }
  .toast.warn {
    --accent: var(--ls-warning);
  }
  .toast.info {
    --accent: var(--ls-info);
  }
  .toast .msg {
    min-width: 0;
  }
  .toast .dismiss {
    flex: none;
    font-size: var(--fs-sm);
    line-height: 1;
    color: var(--ls-text-dim);
  }
  .toast .dismiss:hover {
    color: var(--ls-text);
  }

  /* A failed write is never silent: the frontend cannot know whether the catalog changed, so
     saying nothing would leave the user believing an action happened. */
  .action-error {
    flex: none;
    margin: 0;
    padding: var(--sp-2) var(--sp-3);
    background: var(--ls-surface);
    border-top: 1px solid var(--ls-danger);
    color: var(--ls-danger);
    font-size: var(--fs-sm);
  }

  .statusbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    height: 24px;
    padding: 0 var(--sp-3);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    background: var(--ls-surface);
    border-top: 1px solid var(--ls-border);
  }
  .reload {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    text-decoration: underline;
  }
  .reload:hover:not(:disabled) {
    color: var(--ls-gold);
  }
  .right {
    margin-left: auto;
  }
  .path {
    max-width: 46ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
  }
</style>
