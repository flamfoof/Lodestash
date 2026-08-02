<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import TopBar from "./lib/TopBar.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import Screen from "./lib/Screen.svelte";
  import { SCREENS } from "./lib/routes";
  import { view } from "./lib/nav.svelte";

  type AppInfo = { name: string; version: string; engine_ready: boolean };

  // One real IPC round-trip, so the scaffold proves the Rust<->webview channel rather
  // than only looking right. In `vite dev` outside Tauri there is no host, so fall back.
  let info = $state<AppInfo>({ name: "Lodestash", version: "—", engine_ready: false });
  invoke<AppInfo>("app_info")
    .then((r) => (info = r))
    .catch(() => {});

  let screen = $derived(SCREENS[view.route]);
</script>

<div class="frame">
  <TopBar engineReady={info.engine_ready} />
  <div class="body">
    <Sidebar />
    <main>
      <Screen title={screen.title} milestone={screen.milestone} />
    </main>
    {#if view.inspectorOpen}
      <aside class="inspector">
        <p class="stub">Inspector — lands in B3.</p>
      </aside>
    {/if}
  </div>
  <footer class="statusbar">
    <span>{info.name} {info.version}</span>
    <span class="right">
      {info.engine_ready ? "catalog connected" : "no catalog — shell scaffold"}
    </span>
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
  .inspector {
    width: var(--inspector-w);
    flex: none;
    background: var(--ls-surface);
    border-left: 1px solid var(--ls-border);
    padding: var(--sp-4);
  }
  .stub {
    color: var(--ls-text-dim);
    margin: 0;
  }
  .statusbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    height: 24px;
    padding: 0 var(--sp-3);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    background: var(--ls-surface);
    border-top: 1px solid var(--ls-border);
  }
  .right {
    margin-left: auto;
  }
</style>
