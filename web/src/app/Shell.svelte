<script lang="ts">
  import type { Snippet } from "svelte";
  import Sidebar from "../features/sidebar/Sidebar.svelte";
  import TopBar from "../features/topbar/TopBar.svelte";
  import type { ShellData, ShellEvents } from "./shell";

  /**
   * The application frame: one CSS grid — the top bar across, then rail | main | inspector
   * (spec §9.1; FEATURES §0.1). Grid placement only; the panes are size containers their contents
   * adapt to. Below a 1280 px window the inspector overlays the main pane instead of pushing it
   * (ADR-0009); below the 900 × 600 floor the frame stops shrinking and the page scrolls.
   * Data comes in as props (`ShellData`); intents go out as callbacks — nothing here fetches.
   */
  let {
    data,
    density = "compact",
    children,
    inspector,
    ...events
  }: ShellEvents & {
    data: ShellData;
    density?: "compact" | "cozy" | "large";
    children: Snippet;
    inspector?: Snippet;
  } = $props();
</script>

<div class="shell" data-density={density}>
  <div class="top">
    <TopBar
      mode={data.mode}
      query={data.query}
      category={data.category}
      health={data.health}
      project={data.project}
      device={data.device}
      onmode={events.onmode}
      onquery={events.onquery}
      oncategory={events.oncategory}
      onproject={events.onproject}
    />
  </div>
  <div class="side">
    <Sidebar
      route={data.route}
      badges={data.badges}
      status={data.status}
      libraries={data.libraries}
      onnavigate={events.onnavigate}
      onstatus={events.onstatus}
      onlibrary={events.onlibrary}
      onaddsource={events.onaddsource}
    />
  </div>
  <main class="main" id="main">{@render children()}</main>
  {#if inspector}
    <div class="inspector">{@render inspector()}</div>
  {/if}
</div>

<style>
  .shell {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    grid-template-rows: auto minmax(0, 1fr);
    grid-template-areas:
      "top top top"
      "side main inspector";
    height: 100vh;
    min-width: var(--window-min-w);
    min-height: var(--window-min-h);
  }

  .top {
    grid-area: top;
    min-width: 0;
  }

  .side {
    grid-area: side;
    display: flex;
    min-height: 0;
  }

  .main {
    grid-area: main;
    container: main / size;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .inspector {
    grid-area: inspector;
    container: inspector / size;
    /* The panel's 300 px plus its hairline: the prototype's box is 301 px wide. */
    width: calc(var(--inspector-w) + 1px);
    min-height: 0;
  }

  /* ADR-0009: below 1280 px the inspector overlays the grid rather than pushing it. */
  @media (max-width: 1279.98px) {
    .inspector {
      grid-area: main;
      justify-self: end;
      z-index: var(--z-raised);
    }
  }
</style>
