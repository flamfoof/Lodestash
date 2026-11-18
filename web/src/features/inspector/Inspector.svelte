<script lang="ts">
  import { Button, Chip, IconButton, MiniWave, PanelTitle, Provenance, SectionHeading, SidePanel, StatList, StatusLine, Text, Thumb, TraitChip, TypeMark, type ProvenanceLine } from "../../ui";
  import { intentFor, whyNot, type AssetActionId, type AssetIntent } from "../asset/actions";
  import type { AssetDetail, Collection, PackageRef, ProjectRef, TraitDef } from "../asset/model";
  import { chipOf, importedState, quickAddTraits, statRows, typeLabel, typeTone, userStatus } from "../asset/present";
  import CollectionsPicker from "./CollectionsPicker.svelte";
  import LinkedFiles from "./LinkedFiles.svelte";

  /**
   * The Inspector (UI-SPEC §13): the selected asset's "stat block" — preview, identity, stats,
   * traits, linked files, collections, provenance and actions, top to bottom.
   *
   * **It holds nothing.** Every value on it is derived from `asset` at the moment it is drawn:
   * there is no copy of the tags, the collections or the staged flag to go stale after a write
   * (the review's U3). A control reports what was asked — an intent from `actions.ts`, or a
   * callback — and the panel changes when its owner hands it the changed asset.
   *
   * Composition and layout only; every look is a primitive's.
   */
  let {
    asset,
    collections = [],
    traits = [],
    project,
    anchored = false,
    onintent,
    onclose,
    oncollection,
    onaddtag,
    onopenasset,
    onsearchtag,
    onpack,
  }: {
    asset: AssetDetail;
    /** Every collection, to offer membership in. */
    collections?: readonly Collection[];
    /** The user's trait library: a manual chip's colour, and the quick-add offers. */
    traits?: readonly TraitDef[];
    project: ProjectRef | null;
    /** Mark the parts the contract gate measures. */
    anchored?: boolean;
    onintent?: (intent: AssetIntent) => void;
    onclose?: () => void;
    oncollection?: (id: number, on: boolean) => void;
    /** Apply a trait from the library to this asset. */
    onaddtag?: (name: string) => void;
    onopenasset?: (id: number) => void;
    onsearchtag?: (value: string) => void;
    onpack?: (pack: PackageRef) => void;
  } = $props();

  const status = $derived(userStatus(asset.status));
  const unidentified = $derived(status === "unidentified");
  const ctx = $derived({ targets: [asset], project });
  const imported = $derived(importedState(asset, project));
  const offers = $derived(unidentified ? [] : quickAddTraits(asset, traits));
  const anchor = (name?: string) => (anchored ? name : undefined);

  const provenance = $derived.by(() => {
    const pack = asset.package;
    const lines: ProvenanceLine[] = [
      pack
        ? { label: "Source", value: `${asset.library.name} · ${pack.name}`, onclick: () => onpack?.(pack), title: "Open pack stats" }
        : { label: "Source", value: asset.library.name },
    ];
    // A licence nobody stated has no row: "unknown" is not provenance (UI-SPEC §13.9).
    if (asset.license) lines.push({ label: "License", value: asset.license.name });
    if (imported?.line) lines.push({ value: imported.line, tone: "success" });
    return lines;
  });

  function run(id: AssetActionId) {
    const intent = intentFor(id, ctx);
    if (intent) onintent?.(intent);
  }
</script>

<SidePanel label="Inspector" data-anchor={anchor("inspector")}>
  <Thumb size="preview" src={asset.preview_url} unknown={unidentified && asset.media_type !== "audio"} data-anchor={anchor("inspector.preview")}>
    {#if asset.media_type === "audio"}<MiniWave size="lg" unknown={unidentified} />{/if}
    <span class="full">
      <Button size="pill" onMedia accent="arcane" icon="action-fullsize" iconSize="11" iconBox="8" onclick={() => run("quick-look")} data-anchor={anchor("inspector.fullsize")}>
        full size
      </Button>
    </span>
    <span class="close">
      <IconButton variant="media" icon="action-clear" label="Close inspector" onclick={() => onclose?.()} data-anchor={anchor("inspector.close")} />
    </span>
  </Thumb>

  <div class="body" data-anchor={anchor("inspector.body")}>
    <header>
      <div class="name">
        <TypeMark tone={typeTone(asset.media_type)} size="md" />
        <PanelTitle text={asset.name} unknown={unidentified} data-anchor={anchor("inspector.name")} />
      </div>
      <StatusLine {status} suffix={typeLabel(asset.media_type)} rule data-anchor={anchor("inspector.status")} ruleAttrs={{ "data-anchor": anchor("inspector.rule") }} />
    </header>

    <StatList rows={statRows(asset)} anchors={anchored ? { row: "inspector.stat", key: "inspector.stat.key", value: "inspector.stat.value" } : {}} />

    <section aria-label="Traits">
      <SectionHeading text="TRAITS" textAttrs={{ "data-anchor": anchor("inspector.heading") }} />
      {#if asset.tags.length === 0}
        <Text variant="empty">No traits yet — run Identify.</Text>
      {:else}
        <div class="chips">
          {#each asset.tags as tag, i (tag.source + tag.value)}
            {@const chip = chipOf(tag, traits)}
            <TraitChip
              name={chip.name}
              source={chip.source}
              confidence={chip.confidence}
              color={chip.color}
              icon={chip.icon}
              size="md"
              title="Search for {chip.name}"
              onclick={() => onsearchtag?.(chip.name)}
              data-anchor={anchor(i === 0 ? "inspector.chip.lead" : i === 1 ? "inspector.chip" : undefined)}
            />
          {/each}
        </div>
      {/if}
      {#if offers.length > 0}
        <div class="chips offers">
          {#each offers as trait, i (trait.id)}
            <Chip
              tone="add"
              size="md"
              label="+{trait.name}"
              title={trait.usage || "From your trait library"}
              aria-label="Add trait {trait.name}"
              onclick={() => onaddtag?.(trait.name)}
              data-anchor={i === 0 ? anchor("inspector.trait.add") : undefined}
            />
          {/each}
        </div>
        <div class="note"><Text variant="note">quick-tag from your trait library</Text></div>
      {/if}
    </section>

    {#if asset.links.length > 0}
      <LinkedFiles links={asset.links} {anchored} {onopenasset} />
    {/if}

    {#if !unidentified && collections.length > 0}
      <CollectionsPicker {collections} memberOf={asset.collection_ids} {anchored} ontoggle={oncollection} />
    {/if}

    <Provenance lines={provenance} data-anchor={anchor("inspector.provenance")} />

    <div class="actions">
      {#if unidentified}
        <Button variant="arcane" size="block" icon="action-ai-sieve" iconSize="13" iconBox="11" onclick={() => run("identify")}>Identify now</Button>
      {:else}
        <div class="pair">
          <Button
            variant="outline-gold"
            size="block"
            icon={asset.staged ? "status-identified" : "action-stage"}
            iconSize="13"
            iconBox="11"
            aria-pressed={asset.staged}
            onclick={() => run("stage")}
            data-anchor={anchor("inspector.stage")}
          >
            {asset.staged ? "Staged" : "Stage"}
          </Button>
          <Button
            variant="primary"
            size="block"
            icon={imported?.tone === "active" ? "status-identified" : "import"}
            iconSize="13"
            iconBox="11"
            disabled={whyNot("import", ctx) !== undefined}
            title={whyNot("import", ctx) ?? (imported?.tone === "active" ? "Already in this project — importing again asks first" : undefined)}
            onclick={() => run("import")}
            data-anchor={anchor("inspector.import")}
          >
            {imported?.tone === "active" ? "Imported" : "Import"}
          </Button>
        </div>
        <Button variant="outline-arcane" size="block-sm" icon="action-ai-sieve" iconBox="10" onclick={() => run("identify")} data-anchor={anchor("inspector.reidentify")}>
          Re-identify
        </Button>
      {/if}
    </div>
  </div>
</SidePanel>

<style>
  /* Layout only: where the preview's two controls sit, and the column's rhythm. */
  .full,
  .close {
    position: absolute;
    display: flex;
  }

  .full {
    left: var(--space-12);
    bottom: var(--space-10);
  }

  .close {
    top: var(--space-8);
    right: var(--space-8);
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-14) var(--space-16);
  }

  .name {
    display: flex;
    align-items: center;
    gap: var(--space-8);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-5);
  }

  .offers {
    margin-top: var(--space-7);
  }

  .note {
    margin-top: var(--space-5);
  }

  .actions {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    margin-top: var(--space-2);
  }

  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-8);
  }
</style>
