<script lang="ts">
  import { CATEGORIES, type Category, type Project, type SearchMode } from "../../app/shell";
  import { AppBar, Pill, ProgressBar, SearchField, SegmentedControl, Tabs, Wordmark } from "../../ui";

  /**
   * The top bar (FEATURES §0.2): brand, search mode, search, category tabs, the library-health
   * meter, the project chip and the local-inference chip. Composition and layout only — every look
   * is a primitive's. Data in, intents out; nothing here fetches.
   */
  let {
    mode,
    query,
    category,
    health,
    project,
    device,
    onmode,
    onquery,
    oncategory,
    onproject,
  }: {
    mode: SearchMode;
    query: string;
    category: Category;
    health: { identified: number; total: number };
    project: Project | null;
    device: string;
    onmode?: (mode: SearchMode) => void;
    onquery?: (query: string) => void;
    oncategory?: (category: Category) => void;
    onproject?: () => void;
  } = $props();

  const placeholder = $derived(
    mode === "ai" ? 'describe what you need — e.g. "pickup sound for rocks" — then Enter' : "filter by tag, name, or pack…",
  );
</script>

<AppBar data-anchor="topbar">
  <Wordmark textAttrs={{ "data-anchor": "topbar.wordmark" }} collapse="md" />
  <SegmentedControl
    label="Search mode"
    value={mode}
    onchange={onmode}
    collapse="md"
    data-anchor="topbar.mode"
    anchors={{ on: "topbar.mode.on", off: "topbar.mode.off" }}
    options={[
      { value: "tags", label: "Tags", icon: "search-tags", iconBox: "8" },
      { value: "ai", label: "AI", icon: "search-ai", iconBox: "10", tone: "arcane" },
    ]}
  />
  <SearchField
    value={query}
    {placeholder}
    label={mode === "ai" ? "Ask AI" : "Filter by tag, name or pack"}
    tone={mode === "ai" ? "arcane" : "gold"}
    oninput={(e) => onquery?.(e.currentTarget.value)}
    wellAttrs={{ "data-anchor": "topbar.search" }}
  />
  <Tabs
    label="Asset type"
    value={category}
    onchange={oncategory}
    collapse="lg"
    controls="main"
    anchors={{ on: "topbar.category.on", off: "topbar.category" }}
    tabs={[...CATEGORIES]}
  />
  <div class="end">
    <ProgressBar
      value={health.identified}
      max={health.total}
      label="Identified"
      valueText="{health.identified} / {health.total}"
      data-anchor="topbar.meter"
      labelAttrs={{ "data-anchor": "topbar.meter.label" }}
    />
  </div>
  {#if project}
    <Pill label="{project.name} · {project.engine}" dot={project.color} caret open={false} onclick={() => onproject?.()} data-anchor="topbar.project" />
  {:else}
    <Pill label="Connect a Unity project…" caret open={false} onclick={() => onproject?.()} data-anchor="topbar.project" />
  {/if}
  <Pill label="local · {device}" tone="arcane" icon="action-ai-sieve" iconBox="9" collapse="xl" data-anchor="topbar.hardware" />
</AppBar>

<style>
  /* The meter and the chips after it sit at the bar's right end. */
  .end {
    margin-left: auto;
    flex-shrink: 0;
  }
</style>
