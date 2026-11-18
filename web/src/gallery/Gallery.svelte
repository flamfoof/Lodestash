<script lang="ts">
  import {
    Badge,
    Banner,
    Button,
    Checkbox,
    Chip,
    ConfirmDialog,
    ContextMenu,
    Dialog,
    Divider,
    EmptyState,
    Icon,
    IconButton,
    ICON_NAMES,
    ImportedBadge,
    Input,
    ItemCard,
    Kbd,
    LinkRow,
    ListRow,
    Menu,
    MiniWave,
    Pill,
    Popover,
    ProgressBar,
    Provenance,
    Rail,
    RailHeading,
    SearchField,
    SectionCard,
    SectionHeading,
    SegmentedControl,
    Select,
    SidePanel,
    SizeTier,
    Slider,
    Spinner,
    Stat,
    StatList,
    StatusGem,
    StatusLine,
    Switch,
    Tabs,
    Text,
    TextLink,
    Thumb,
    Toast,
    ToggleButton,
    Toolbar,
    tooltip,
    TraitChip,
    TreeRow,
    TypeMark,
    Wordmark,
  } from "../ui";

  /*
   * Every primitive, once. A specimen that reproduces a design-contract anchor carries
   * `data-anchor="<name>"` and the text the prototype shows there, so the gate
   * (web/e2e/gate.mjs --gallery) can measure it against docs/design/contract.json. Positions are
   * not compared here; sizes, type, colour, fill, edges and spacing are.
   */

  let mode = $state<"tags" | "ai">("tags");
  let category = $state<"all" | "model" | "sound" | "image">("all");
  let sort = $state("default");
  let favorites = $state(false);
  let density = $state<"compact" | "cozy" | "large">("cozy");
  let query = $state("");

  let filtersButton = $state<HTMLElement>();
  let filtersOpen = $state(false);
  let menuButton = $state<HTMLElement>();
  let menuOpen = $state(false);
  let contextEvent = $state<MouseEvent | null>(null);
  let dialogOpen = $state(false);
  let confirmOpen = $state(false);
  let switchOn = $state(true);
  let checked = $state(false);
  let cap = $state(8);
  let expanded = $state(true);
  let lastAction = $state("—");
  let view = $state<"grid" | "list">("grid");
  let cardPicked = $state(false);
  let cardStaged = $state(false);
  let inCollection = $state(false);
</script>

<main class="gallery">
  <h1>Gallery</h1>

  <section>
    <h2>Top bar</h2>
    <div class="row">
      <Wordmark textAttrs={{ "data-anchor": "topbar.wordmark" }} />
      <SegmentedControl
        label="Search mode"
        bind:value={mode}
        data-anchor="topbar.mode"
        anchors={{ on: "topbar.mode.on", off: "topbar.mode.off" }}
        options={[
          { value: "tags", label: "Tags", icon: "search-tags", iconBox: "8" },
          { value: "ai", label: "AI", icon: "search-ai", iconBox: "10", tone: "arcane" },
        ]}
      />
      <Tabs
        label="Asset type"
        bind:value={category}
        anchors={{ on: "topbar.category.on", off: "topbar.category" }}
        tabs={[
          { value: "all", label: "All", icon: "type-all", iconBox: "13" },
          { value: "model", label: "Models", icon: "type-model", iconBox: "9", tone: "model" },
          { value: "sound", label: "Sounds", icon: "type-sound", iconSize: "10", iconBox: "6", tone: "sound" },
          { value: "image", label: "Images", icon: "type-image", iconBox: "10", tone: "image" },
        ]}
      />
      <ProgressBar
        value={14}
        max={19}
        label="Identified"
        valueText="14 / 19"
        data-anchor="topbar.meter"
        labelAttrs={{ "data-anchor": "topbar.meter.label" }}
      />
      <Pill label="MyRPG · Unity 6" dot="var(--ls-success)" caret open={false} onclick={() => (lastAction = "project")} data-anchor="topbar.project" />
      <Pill label="local · RTX 4070" tone="arcane" icon="action-ai-sieve" iconBox="9" data-anchor="topbar.hardware" />
    </div>
    <div class="row wide">
      <SearchField bind:value={query} placeholder="filter by tag, name, or pack…" tone={mode === "ai" ? "arcane" : "gold"} />
    </div>
  </section>

  <section>
    <h2>Rail</h2>
    <div class="row top">
      <Rail aria-label="Specimen rail">
        <ListRow label="Library" icon="nav-library" current data-anchor="sidebar.nav.on" />
        <ListRow label="Identify" icon="nav-identify">
          {#snippet trailing()}<Badge label="5 unidentified" data-anchor="sidebar.nav.badge">5</Badge>{/snippet}
        </ListRow>
        <ListRow label="Collections" icon="nav-collections" data-anchor="sidebar.nav" />
        <ListRow label="Unity Assets" icon="unitypackage" />
        <Divider />
        <RailHeading text="STATUS" data-anchor="sidebar.heading" />
        <ListRow variant="facet" label="All" dot="var(--ls-text-dim)" count={19} current />
        <ListRow variant="facet" label="Unidentified" dot="var(--ls-status-unidentified)" count={5} data-anchor="sidebar.facet" />
        <div class="rail-foot">
          <Button variant="dashed" data-anchor="sidebar.addSource">
            {#snippet leading()}<span>+</span>{/snippet}
            Add source…
          </Button>
        </div>
      </Rail>
      <div class="tree">
        <TreeRow label="Synty" count={3} {expanded} ontoggle={() => (expanded = !expanded)} />
        {#if expanded}
          <TreeRow label="POLYGON City Pack" level={1} icon="unitypackage" count={1240} current />
          <TreeRow label="Forest Pack" level={1} icon="folder" count={2} />
        {/if}
      </div>
    </div>
  </section>

  <section>
    <h2>Library toolbar</h2>
    <Banner figure={5} action={{ label: "Open Identify mode →", onclick: () => (lastAction = "identify") }} actionAttrs={{ "data-anchor": "library.banner.action" }}>
      assets are Unidentified — invisible to AI search until appraised.
    </Banner>
    <Toolbar label="Library view">
      <Text>19 assets · all libraries</Text>
      <span class="grow"></span>
      <Text variant="hint">Space quick-looks · <Kbd>F</Kbd> favourite</Text>
      <Select
        label="Sort"
        bind:value={sort}
        data-anchor="library.sort"
        options={[
          { value: "default", label: "sort · default" },
          { value: "name", label: "sort · name A–Z" },
          { value: "sizeDesc", label: "sort · largest first" },
          { value: "sizeAsc", label: "sort · smallest first" },
          { value: "tris", label: "sort · tri count" },
        ]}
      />
      <ToggleButton bind:pressed={favorites} icon="nav-favorites" iconBox="13" title="Show only favorites" data-anchor="library.favorites">Favorites</ToggleButton>
      <span bind:this={filtersButton}>
        <Button
          icon="action-filters"
          iconBox="9"
          aria-expanded={filtersOpen}
          aria-haspopup="dialog"
          aria-controls="filters-panel"
          onclick={() => (filtersOpen = !filtersOpen)}
          data-anchor="library.filters"
        >
          Filters
        </Button>
      </span>
      <SegmentedControl
        label="Grid size"
        size="sm"
        bind:value={density}
        data-anchor="library.density"
        anchors={{ on: "library.density.on" }}
        options={[
          { value: "compact", title: "Compact", icon: "grid-compact", iconSize: "10", iconBox: "7" },
          { value: "cozy", title: "Cozy", icon: "grid-cozy", iconSize: "10", iconBox: "10" },
          { value: "large", title: "Large", icon: "grid-large", iconSize: "10", iconBox: "10" },
        ]}
      />
      <SegmentedControl
        label="View"
        size="sm-wide"
        bind:value={view}
        options={[
          { value: "grid", title: "Grid view", icon: "view-grid", iconBox: "12" },
          { value: "list", title: "List view", icon: "view-list", iconBox: "11" },
        ]}
      />
    </Toolbar>
    <Popover bind:open={filtersOpen} anchor={filtersButton} id="filters-panel" label="Filter library">
      <div class="stack">
        <Input label="TRAIT" placeholder="search traits…" size="sm" />
        <div class="row">
          <Chip label="rock" tone="ai" size="md" selected onclick={() => {}} />
          <Chip label="moss" tone="ai" size="md" onclick={() => {}} />
        </div>
      </div>
    </Popover>
  </section>

  <section>
    <h2>Card parts</h2>
    <div class="row media">
      <TraitChip name="animal" source="human" onclick={() => (lastAction = "animal")} data-anchor="card.chip.lead" />
      <TraitChip name="lowpoly" source="ai" confidence={0.94} onclick={() => (lastAction = "lowpoly")} data-anchor="card.chip" />
      <Chip label="+1" tone="neutral" onclick={() => (lastAction = "more")} data-anchor="card.chip.more" />
      <Chip label="???" tone="unknown" />
      <TraitChip name="hero-prop" source="human" color="var(--ls-type-sound)" />
      <SizeTier tier="L" size="4.1 MB" compact data-anchor="card.size" />
      <SizeTier tier="L" size="4.1 MB" />
      <SizeTier tier="S" size="922 KB" variant="text" />
      <ImportedBadge label="in MyRPG" />
      <ImportedBadge label="in 2 others" tone="others" />
      <ImportedBadge label="older copy" tone="stale" />
      <Button size="xs" onMedia accent="arcane" icon="action-fullsize" iconSize="10" iconBox="10" aria-label="Quick Look" title="Full-size preview" />
      <Button size="xs" onMedia active>✓ staged</Button>
      <IconButton variant="round" icon="play" iconSize="11" label="Play" />
      <MiniWave />
      <MiniWave unknown />
      <Button size="xs" onMedia data-anchor="card.stage">+ stage</Button>
      <IconButton variant="tile" icon="nav-favorites" iconSize="10" label="Favorite" pressed={false} data-anchor="card.favorite" />
      <IconButton variant="tile" icon="nav-favorites" iconSize="10" label="Favorite" pressed />
      <StatusGem status="confirmed" data-anchor="card.status" />
      <StatusGem status="identified" />
      <StatusGem status="unidentified" />
    </div>
  </section>

  <section>
    <h2>Item card</h2>
    <div class="row top">
      <div class="card-slot" role="grid" aria-label="Specimen cards">
        <div role="row" class="card-row">
          <ItemCard status="confirmed" title="Beaver_LowPoly" tabStop pin={{ topStart: cardPicked, bottomEnd: cardStaged }} onopen={() => (lastAction = "open")}>
            {#snippet media()}<Thumb />{/snippet}
            {#snippet topStart()}<Checkbox bind:checked={cardPicked} label="Select for bulk actions" hideLabel tabindex={-1} />{/snippet}
            {#snippet topEnd()}
              <IconButton variant="tile" icon="nav-favorites" iconSize="10" label="Favorite" pressed={false} tabindex={-1} />
              <StatusGem status="confirmed" />
            {/snippet}
            {#snippet bottomStart()}<SizeTier tier="L" size="4.1 MB" compact /><ImportedBadge label="in MyRPG" />{/snippet}
            {#snippet bottomEnd()}
              <Button size="xs" onMedia accent="arcane" icon="action-fullsize" iconSize="10" iconBox="10" aria-label="Quick Look" tabindex={-1} />
              <Button size="xs" onMedia active={cardStaged} tabindex={-1} onclick={() => (cardStaged = !cardStaged)}>{cardStaged ? "✓ staged" : "+ stage"}</Button>
            {/snippet}
            {#snippet mark()}<TypeMark tone="model" label="Model" />{/snippet}
            {#snippet meta()}<TextLink tabindex={-1} onclick={() => (lastAction = "pack")}>Forest Pack</TextLink> · 1.2k tris · 4.1 MB{/snippet}
            {#snippet chips()}
              <TraitChip name="animal" source="human" />
              <TraitChip name="lowpoly" source="ai" confidence={0.94} />
            {/snippet}
          </ItemCard>
          <ItemCard status="unidentified" title="audio_23_final" selected>
            {#snippet media()}
              <Thumb><IconButton variant="round" icon="play" iconSize="11" label="Play" tabindex={-1} /><MiniWave unknown /></Thumb>
            {/snippet}
            {#snippet topEnd()}<StatusGem status="unidentified" />{/snippet}
            {#snippet mark()}<TypeMark tone="sound" label="Sound" />{/snippet}
            {#snippet meta()}Ambience Bundle · 0:00.6 · 44.1 kHz{/snippet}
            {#snippet chips()}<Chip label="???" tone="unknown" />{/snippet}
          </ItemCard>
        </div>
      </div>
    </div>
  </section>

  <section>
    <h2>Inspector parts</h2>
    <div class="row top">
      <div class="panel-slot">
        <SidePanel label="Specimen inspector">
          <Thumb size="preview" />
          <div class="panel-body">
            <div>
              <div class="row tight"><TypeMark tone="model" size="md" label="Model" /><b>Beaver_LowPoly</b></div>
              <StatusLine status="confirmed" suffix="Model" rule />
            </div>
            <StatList
              rows={[
                ["Format", "FBX"],
                ["Tris", "1,248"],
                ["Size", "4.1 MB"],
              ]}
            />
            <div>
              <SectionHeading text="TRAITS" />
              <div class="row tight">
                <TraitChip name="animal" source="human" size="md" />
                <TraitChip name="lowpoly" source="ai" confidence={0.94} size="md" />
                <TraitChip name="Props" source="imported" size="md" />
                <Chip tone="add" size="md" label="+hero-prop" onclick={() => (lastAction = "add trait")} />
              </div>
              <Text variant="note">quick-tag from your trait library</Text>
              <Text variant="empty">No traits yet — run Identify.</Text>
            </div>
            <div>
              <SectionHeading text="LINKED FILES" caption="parsed from FBX bindings" />
              <div class="stack tight">
                <LinkRow name="M_Beaver_Body" kind="material" state="in_pack" />
                <LinkRow name="T_Beaver_Albedo.png" kind="texture" state="in_library" indent onclick={() => (lastAction = "open link")} />
                <LinkRow name="T_Sword_Metallic.png" kind="texture" state="missing" indent />
                <LinkRow name="T_Rope_Albedo.png" kind="texture" state="ignored" indent />
              </div>
              <Text variant="warning">missing links are skipped on export</Text>
            </div>
            <div>
              <SectionHeading text="COLLECTIONS" />
              <Chip tone="toggle" size="md" label="{inCollection ? '✓' : '+'}Forest ambience" selected={inCollection} onclick={() => (inCollection = !inCollection)} />
            </div>
            <Provenance
              lines={[
                { label: "Source", value: "Synty · Forest Pack", onclick: () => (lastAction = "pack") },
                { label: "License", value: "Synty EULA" },
                { value: "imported in MyRPG → Assets/Imported/", tone: "success" },
              ]}
            />
            <div class="stack">
              <div class="pair">
                <Button variant="outline-gold" size="block" icon="action-stage" iconSize="13" iconBox="13">Stage</Button>
                <Button variant="primary" size="block" icon="import" iconSize="13" iconBox="11">Import</Button>
              </div>
              <Button variant="outline-arcane" size="block-sm" icon="action-ai-sieve" iconBox="10">Re-identify</Button>
              <Button variant="arcane" size="block" icon="action-ai-sieve" iconSize="13" iconBox="11">Identify now</Button>
            </div>
          </div>
        </SidePanel>
      </div>
      <Button size="pill" onMedia accent="arcane" icon="action-fullsize" iconSize="11" iconBox="11">full size</Button>
      <IconButton variant="media" icon="action-clear" label="Close" />
    </div>
  </section>

  <section>
    <h2>Buttons</h2>
    <div class="row">
      <Button variant="primary" size="lg">+ Create</Button>
      <Button variant="secondary">Secondary</Button>
      <Button variant="ghost">Ghost</Button>
      <Button variant="danger">Move duplicate to Trash</Button>
      <Button variant="link">Open Identify mode →</Button>
      <Button size="sm" icon="action-copy" iconSize="10">Copy</Button>
      <Button disabled>Disabled</Button>
      <span {@attach tooltip("A tooltip, on hover and on focus")}><Button>With tooltip</Button></span>
    </div>
  </section>

  <section>
    <h2>Fields and settings</h2>
    <div class="row">
      <Input label="FOLDER" placeholder="folder path or asset name…" />
      <Checkbox bind:checked label="Select for bulk actions" />
      <Switch bind:checked={switchOn} label="Enable MCP server" />
      <span class="slider"><Slider bind:value={cap} min={1} max={16} label="Preview cache limit" valueText="{cap} GB" /></span>
    </div>
    <div class="cards">
      <SectionCard heading="ENGINE BRIDGE" description="Copies picks into the open Unity project.">
        {#snippet actions()}<Button size="sm">Reset defaults</Button>{/snippet}
        <ProgressBar value={62} label="Cache used" valueText="5.0 / 8 GB" tone="arcane" />
      </SectionCard>
      <Stat value="19" label="Assets" sub="across 4 libraries" tone="gold" />
      <Stat value="5" label="Unidentified" sub="invisible to AI search" tone="warning" />
    </div>
  </section>

  <section>
    <h2>Overlays</h2>
    <div class="row">
      <Button onclick={() => (dialogOpen = true)}>Open dialog</Button>
      <Button variant="danger" onclick={() => (confirmOpen = true)}>Delete file from disk…</Button>
      <span bind:this={menuButton}>
        <Button aria-haspopup="menu" aria-expanded={menuOpen} onclick={() => (menuOpen = true)}>Open menu</Button>
      </span>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="context-target" oncontextmenu={(e) => (contextEvent = e)}>Right-click here</div>
      <span class="last">last action: {lastAction}</span>
    </div>
    <Dialog bind:open={dialogOpen} title="Add a source" description="Watch a folder, or index Unity Asset Store packages in place.">
      <Input label="FOLDER" placeholder="G:/GameDev/AssetLibrary" />
      {#snippet footer()}
        <Button onclick={() => (dialogOpen = false)}>Cancel</Button>
        <Button variant="primary" data-autofocus onclick={() => (dialogOpen = false)}>Watch folder</Button>
      {/snippet}
    </Dialog>
    <ConfirmDialog
      bind:open={confirmOpen}
      title="Delete file from disk?"
      message="This removes the original file. Lodestash never does this without asking."
      confirmLabel="Delete file"
      destructive
      onconfirm={() => (lastAction = "deleted")}
    />
    <Menu
      bind:open={menuOpen}
      anchor={menuButton}
      label="Asset actions"
      onselect={(id) => (lastAction = id)}
      items={[
        { id: "open", label: "Open", shortcut: "Enter" },
        { id: "look", label: "Quick Look", icon: "action-fullsize", shortcut: "Space" },
        { separator: true },
        { id: "import", label: "Import…", icon: "import", disabled: true, hint: "No project is open" },
        { id: "trash", label: "Move to Trash", danger: true, shortcut: "Del" },
      ]}
    />
    <ContextMenu
      bind:event={contextEvent}
      label="Asset actions"
      onselect={(id) => (lastAction = id)}
      items={[
        { id: "copy-name", label: "Copy name", icon: "action-copy" },
        { id: "copy-path", label: "Copy path" },
      ]}
    />
  </section>

  <section>
    <h2>Feedback</h2>
    <div class="stack narrow">
      <Toast tone="gold" message="★ Added to Favorites" />
      <Toast tone="success" message="Removed 3 tags" action={{ label: "Undo", onclick: () => (lastAction = "undo") }} />
      <Toast tone="danger" message="The engine stopped responding" ondismiss={() => {}} />
      <Spinner label="Indexing" />
    </div>
    <EmptyState icon="search" title="No assets match" line="Clear a filter or search a wider library.">
      {#snippet action()}<Button>Clear filters</Button>{/snippet}
    </EmptyState>
  </section>

  <section>
    <h2>Icons</h2>
    <ul class="icons">
      {#each ICON_NAMES as name (name)}
        <li title={name}><Icon {name} size="20" /></li>
      {/each}
    </ul>
  </section>
</main>

<style>
  /* Layout of the specimen sheet only; every look comes from the primitives. */
  .gallery {
    display: flex;
    flex-direction: column;
    gap: var(--space-30);
    padding: var(--space-20);
  }

  h1 {
    margin: 0;
    font-family: var(--ff-display);
    font-size: var(--fs-20);
    color: var(--ls-gold);
  }

  h2 {
    margin: 0 0 var(--space-10);
    font-size: var(--fs-10);
    font-weight: var(--fw-600);
    letter-spacing: var(--track-1_2);
    color: var(--ls-text-dim);
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-14);
  }

  .row.top {
    align-items: flex-start;
  }

  .wide {
    margin-top: var(--space-14);
  }

  .media {
    padding: var(--space-14);
    background: var(--ls-thumb);
  }

  .rail-foot {
    padding: var(--space-4) var(--space-12) 0;
  }

  .tree {
    width: var(--inspector-w);
  }

  .stack {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
  }

  .narrow {
    align-items: flex-start;
  }

  .cards {
    display: grid;
    grid-template-columns: 2fr 1fr 1fr;
    gap: var(--space-12);
    margin-top: var(--space-14);
  }

  .grow {
    flex: 1;
  }

  .tight {
    gap: var(--space-5);
  }

  .card-slot {
    width: calc(var(--card-min-cozy) * 2 + var(--space-12) + var(--space-24));
  }

  .card-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-12);
  }

  .panel-slot {
    width: var(--inspector-w);
    height: calc(var(--inspector-w) * 2);
  }

  .panel-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-14) var(--space-16);
  }

  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-8);
  }

  .slider {
    width: var(--meter-w);
  }

  .context-target {
    padding: var(--space-8) var(--space-14);
    border: 1px dashed var(--ls-border-strong);
    border-radius: var(--r-6);
    color: var(--ls-text-dim);
  }

  .last {
    color: var(--ls-text-dim);
    font-size: var(--fs-11);
  }

  .icons {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-10);
    list-style: none;
    padding: 0;
    margin: 0;
    color: var(--ls-text-muted);
  }
</style>
