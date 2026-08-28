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
  import PackStats from "./lib/PackStats.svelte";
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
    pickFolder,
    exportAssets,
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
    recentOps,
    undo as undoOp,
    refreshSourceHealth,
  } from "./lib/api";
  import { refreshTraitColors } from "./lib/traits.svelte";
  import Onboarding from "./lib/Onboarding.svelte";
  import SourceChooser from "./lib/SourceChooser.svelte";
  import ImportDialog from "./lib/ImportDialog.svelte";
  import { push, dismiss, act, freeze, thaw, toasts, overflow } from "./lib/toasts.svelte";
  import { sources } from "./lib/sources.svelte";
  import { player } from "./lib/audio.svelte";

  let status = $state<LibraryStatus | null>(null);
  let reloading = $state(false);

  /**
   * UI-SPEC §13.2: "switching … screens stops playback". Leaving the Library with a sound still
   * running is audio with no visible source and no way to stop it — the transport that owns it is
   * no longer on screen.
   *
   * Deliberately **not** extended to switching *assets*: §2.5 wants a card to keep playing "while
   * that card is the active audio" so several sounds can be auditioned in a row, and the one-element
   * player already guarantees only one plays at a time. Stopping on every selection change would
   * make the grid transport unusable for the thing it exists to do.
   */
  $effect(() => {
    view.route;
    return () => player.stop();
  });

  // One mirror of source availability for everything below the fold (UI-SPEC §11.1). `status` is
  // assigned from eight places here — open, navigation, pump events, reload, drops — so this
  // watches the value rather than each assignment: one of those is always the one someone forgets.
  $effect(() => {
    sources.sync(status);
  });

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

  /**
   * Source health, once at open (UI-SPEC §11.1). Deliberately fire-and-forget and *after* the first
   * status read: a slow network root can take seconds to answer, and blocking the first paint on it
   * would trade a real problem for a worse one. The re-read below picks up whatever it found.
   */
  refreshSourceHealth()
    .then(() => libraryStatus())
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
    // "Press again", not "Click again": the same arm/confirm now backs the Delete key, and a
    // prompt that names the wrong gesture is a prompt people stop reading.
    push("Press again to confirm — moving to Trash", "warn");
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

  /**
   * Pack stats (UI-SPEC §15). Owned here rather than by LibraryScreen, because the Inspector
   * links to it too (§13.9's provenance row) and the Inspector is App's child. One modal, one
   * piece of state — two of each is how they end up disagreeing about whether one is open.
   */
  let packStats = $state<string | null>(null);

  let confirming = $state(false);
  let actionError = $state<string | null>(null);

  /**
   * Run a write, surface any failure, and make every screen re-ask its question. On success,
   * `ok` (when given) turns the result into a confirmation toast (B10c) — failures stay on the
   * action-error bar, never a toast.
   */
  async function write(
    run: () => Promise<unknown>,
    ok?: (changed: number) => string | null,
    opts?: { undoable?: boolean },
  ) {
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
        if (msg) push(msg, "ok", opts?.undoable ? await undoAction() : undefined);
      }
    } catch (e) {
      actionError = String(e);
    }
  }

  /**
   * Pin the op this write just recorded to the toast that announces it (D8).
   *
   * The id is captured **now**, not read at click time: with two toasts on screen, an Undo that
   * reached for the head of the log would reverse whichever action was newest — silently, and with
   * a confirmation saying it worked. Only the ops the catalog actually records are undoable
   * (trash, collection create, bulk tag), so no head means no action slot rather than a button
   * that lies.
   *
   * ponytail: reads the head after the write instead of threading the id back through
   * `WriteResult` — writes serialise through the one writer thread and the UI awaits each one, so
   * the head is this write's op. Two genuinely concurrent writes could cross; thread the id
   * through `WriteResult` if that ever shows up.
   */
  async function undoAction() {
    try {
      const [head] = await recentOps(1);
      if (!head) return undefined;
      return {
        label: "Undo",
        run: async () => {
          await write(() => undoOp(head.id), () => "Undone");
        },
      };
    } catch {
      return undefined;
    }
  }

  // ---- Staging tray (I6) -------------------------------------------------
  // The tray's ids live here, loaded from the backend once and re-read after every mutation.
  // Cards reach in through Svelte context (set below) rather than a prop chain that would have to
  // thread LibraryScreen → Card; the tray itself gets ids + handlers as plain props.
  const STAGING_CTX = "ls:staging";
  let stagedIds = $state<number[]>([]);
  /**
   * Re-read the tray after any catalog write, rather than only at startup.
   *
   * This was a one-shot `staged()` at init, so the tray only ever changed when *it* was the thing
   * clicked. Trashing a staged asset left its tile sitting there — UI-SPEC §7 says a trashed
   * asset leaves staging everywhere, and `Library::staged_ids` now enforces that on the read, but
   * nothing asked again for the answer.
   *
   * **Not** a fix for the per-project tray of UI-SPEC §16.2: the shell's shortlist is still one
   * in-memory list that no project owns (the open half of the export-history card in
   * `KANBAN-SVELTE.md`), so switching project does not swap it — there is only one. This effect
   * is what will make the swap show up once there is something to swap.
   */
  $effect(() => {
    view.dataVersion;
    let stale = false;
    staged()
      .then((ids) => {
        if (!stale) stagedIds = ids;
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  });

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
      const p = await pickFolder();
      if (p) importRoot = p;
    } catch (e) {
      actionError = String(e);
    }
  }

  // Re-pick the folder from inside the open import dialog (the parent owns the path).
  async function repickImportFolder() {
    try {
      const p = await pickFolder();
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
    const inText =
      e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;

    // `/` focuses the search box (DESIGN-GUIDELINES §9 keyboard nav). Missing until 2026-08-26,
    // along with Delete below — both are named there and neither was in UI-SPEC §0.4's table,
    // which is the table someone implementing the keyboard map reads. §0.4 now carries them.
    if (e.key === "/" && !inText && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const box = document.querySelector<HTMLInputElement>('[data-testid="search-input"]');
      if (box) {
        e.preventDefault();
        box.focus();
        box.select();
      }
      return;
    }

    // Delete — move the selection to Trash, through the same arm/confirm gate the context menu
    // uses, so the §11.7 "Confirm before moving to Trash" setting governs the key too. Acts on
    // the **lead** only, not the pick set: the bulk bar deliberately offers no trash action, and
    // a key that can bin fifty assets at once should not arrive without a bar that says so.
    // Inert on the Trash screen — those rows are already there, and Restore is their action.
    if (e.key === "Delete" && !inText && view.route !== "trash" && view.selected !== null) {
      const id = view.selected;
      e.preventDefault();
      confirmTrash(() =>
        write(() => setTrashed([id], true), () => "Moved to Trash", { undoable: true }).then(
          afterRemoval,
        ),
      );
      return;
    }

    // F — **toggle** the pin on the selection (UI-SPEC §0.3). It set `true` unconditionally until
    // 2026-08-26, so the key could only ever add: pressing it on a starred asset re-wrote the same
    // row and toasted "Added to Favorites" a second time. The card's ☆/★ tile and the context menu
    // both toggled, so the same gesture had two different meanings depending on where you made it.
    //
    // The current value comes from `favIntent` when there is one, not from `selectedDetail`.
    // `selectedDetail` is refetched *after* the write lands, so two presses inside that round trip
    // — which is one key-repeat, never mind a fast hand — would both read the pre-write value and
    // both write the same thing. `favIntent` records what this key last asked for, so the second
    // press answers the first rather than racing it.
    if (e.key.toLowerCase() === "f" && !e.ctrlKey && !e.metaKey && !e.altKey) {
      if (inText) return;
      const id = view.selected;
      if (id == null) return;
      e.preventDefault();
      const on = favIntent?.id === id ? favIntent.on : (selectedDetail?.favorite ?? false);
      favIntent = { id, on: !on };
      write(
        () => setFavorite([id], !on),
        () => (on ? "Removed from Favorites" : "Added to Favorites"),
      );
    }
  }

  /**
   * What `F` last asked for, and for which asset — cleared once the catalog answers with it, or
   * whenever the selection moves. Nothing else writes this: the card's ☆ tile and the context
   * menu both read the row they are drawn from, which is already current.
   */
  let favIntent = $state<{ id: number; on: boolean } | null>(null);
  $effect(() => {
    const d = selectedDetail;
    if (!d || favIntent == null) return;
    if (d.asset_id !== favIntent.id || d.favorite === favIntent.on) favIntent = null;
  });

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
      confirmTrash(() =>
        write(() => setTrashed([id], true), () => "Moved to Trash", { undoable: true }),
      );
    } else if (action === "import") {
      importToProject([id]);
    } else if (action === "export") {
      exportToFolder([id]);
    }
  }

  /**
   * **Export to folder** (UI-SPEC §16.2) — the level-1 way out, and until the Unity plugin exists
   * the only one: `importToProject` needs a registered project whose plugin drains the queue.
   *
   * The native folder dialog is the confirmation. Nothing else prompts, because picking a
   * destination is already a deliberate act, and a second "are you sure" would train the user to
   * click through the one that matters.
   *
   * The toast reports what actually happened rather than that the request was accepted: skipped
   * picks are named by *reason*, since an in-pack asset needs an extract and an unreachable one
   * needs its drive back.
   */
  async function exportToFolder(ids: number[]) {
    if (ids.length === 0) return;
    let dest: string | null = null;
    try {
      dest = await pickFolder(`Export ${ids.length} asset${ids.length === 1 ? "" : "s"} to…`);
    } catch (e) {
      push(String(e), "err");
      return;
    }
    if (!dest) return; // cancelled — not a failure, and not worth a toast
    try {
      const r = await exportAssets(ids, dest);
      const skipped: string[] = [];
      if (r.in_pack > 0) skipped.push(`${r.in_pack} still in a pack`);
      if (r.unreachable > 0) skipped.push(`${r.unreachable} unreachable`);
      const extra = r.dependencies_added > 0 ? ` (+${r.dependencies_added} linked)` : "";
      if (r.written === 0) {
        push(`Nothing was exported — ${skipped.join(", ") || "no files to copy"}`, "warn");
      } else if (skipped.length > 0) {
        push(`Exported ${r.written}${extra} — skipped ${skipped.join(", ")}`, "warn");
      } else {
        push(`Exported ${r.written} file${r.written === 1 ? "" : "s"}${extra} to ${dest}`);
      }
    } catch (e) {
      push(String(e), "err");
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
              write(
                () => setTrashed(ids, true),
                (n) => (n === 1 ? "Moved to Trash" : `Moved ${n} assets to Trash`),
                { undoable: true },
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
          onpack={(name) => (packStats = name)}
          onrestore={(id) =>
            write(() => setTrashed([id], false), () => "Restored from Trash", { undoable: true }).then(
              afterRemoval,
            )
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
        onpack={(name) => (packStats = name)}
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
            write(() => setTrashed([view.selected!], true), () => "Moved to Trash", { undoable: true }).then(
              afterRemoval,
            ),
          )
        }
        onimport={() => view.selected !== null && importToProject([view.selected])}
        onexport={() => view.selected !== null && exportToFolder([view.selected])}
        onrestore={() =>
          write(
            () => setTrashed([view.selected!], false),
            () => "Restored from Trash",
            { undoable: true },
          ).then(afterRemoval)
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
    onexport={(ids) => exportToFolder(ids)}
  />
  <JobTray />

  <!-- UI-SPEC §15, reached from a pack-name link anywhere: a card, a list row, or the
       Inspector's provenance row. One owner, so two of them can never both be open. -->
  <PackStats name={packStats} onclose={() => (packStats = null)} />

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
    <!-- Onboarding indexes folders now (UI-SPEC §1), so the shell has to re-read after it —
         otherwise the library behind the panel is the empty one the user just fixed. -->
    <Onboarding
      onclose={() => (onboardingOpen = false)}
      onindexed={async () => {
        try {
          status = await libraryStatus();
        } catch {
          /* the next navigation re-asks */
        }
        view.dataVersion += 1;
      }}
    />
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
    <!-- Reaching for Undo must not be a race against the toast offering it, so the whole stack
         freezes while it is hovered or focused (DESIGN-GUIDELINES §7). -->
    <div
      class="toasts"
      data-testid="toast-host"
      aria-live="polite"
      role="presentation"
      onmouseenter={freeze}
      onmouseleave={thaw}
      onfocusin={freeze}
      onfocusout={thaw}
    >
      {#if overflow.count > 0}
        <div class="toast more" data-testid="toast-overflow">
          <span class="msg">+{overflow.count} more</span>
        </div>
      {/if}
      {#each toasts as t (t.id)}
        <div class="toast {t.kind}" data-testid="toast" role="status">
          <span class="msg">{t.msg}</span>
          {#if t.action}
            <button class="act" data-testid="toast-action" onclick={() => act(t.id)}>
              {t.action.label}
            </button>
          {/if}
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
  /* One action slot, right of the message: Undo when it is reversible, Show when the useful next
     move is to look at what failed. Never both. */
  .toast .act {
    flex: none;
    margin-left: auto;
    padding: 0;
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--ls-gold);
    background: none;
    border: 0;
    border-bottom: 1px solid color-mix(in srgb, var(--ls-gold) 50%, transparent);
    cursor: pointer;
  }
  .toast .act:hover {
    color: var(--ls-gold-bright);
    border-bottom-color: var(--ls-gold-bright);
  }
  .toast.more {
    --accent: var(--ls-border-strong);
    color: var(--ls-text-dim);
    font-size: var(--fs-xs);
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
