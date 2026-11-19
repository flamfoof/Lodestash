<script lang="ts">
  import { Button, Checkbox, Chip, IconButton, ImportedBadge, ItemCard, MiniWave, SizeTier, StatusGem, TextLink, Thumb, TraitChip, TypeMark } from "../../ui";
  import type { AssetActionId } from "./actions";
  import type { AssetSummary, PackageRef, ProjectRef, SizeThresholds, TraitDef } from "./model";
  import { chipOf, formatBytes, importedState, sizeTier, statLine, typeLabel, typeTone, userStatus } from "./present";

  /**
   * One asset as a card (UI-SPEC §2.5): composition and wiring only — every look is a primitive's.
   *
   * The card is a container (`ItemCard`, a grid cell). Its primary action is the name; the pick
   * box, ☆, ▶, ⤢, stage, the pack link and the chips are real buttons beside it, all
   * `tabindex="-1"`: the grid owns how the keyboard reaches them (AssetGrid). A button here never
   * opens the card, and none of them builds an intent — they name an action (`onaction`) and the
   * owner asks `actions.ts` for it.
   *
   * Everything shown is derived from `asset`; the only state kept here is whether the chip row is
   * expanded, which is nobody else's business.
   */
  let {
    asset,
    selected = false,
    picked = false,
    picking = false,
    tabStop = false,
    project,
    traits = [],
    thresholds,
    anchored = false,
    row,
    col,
    onopen,
    onpick,
    onaction,
    onmenu,
    onpack,
    ontag,
  }: {
    asset: AssetSummary;
    /** The lead: the gold ring, what the inspector describes. */
    selected?: boolean;
    /** In the pick set. */
    picked?: boolean;
    /** Anything is picked: the pick boxes stay visible. */
    picking?: boolean;
    tabStop?: boolean;
    project: ProjectRef | null;
    traits?: readonly TraitDef[];
    thresholds: SizeThresholds;
    /** Mark this card's parts for the contract gate (the grid marks its first card). */
    anchored?: boolean;
    /** The card's place in the grid, 1-based: it says so itself (AssetGrid explains why). */
    row?: number;
    col?: number;
    onopen?: (event: MouseEvent) => void;
    onpick?: () => void;
    onaction?: (id: AssetActionId) => void;
    onmenu?: (event: MouseEvent) => void;
    onpack?: (pack: PackageRef) => void;
    ontag?: (value: string) => void;
  } = $props();

  const VISIBLE_CHIPS = 3;
  let expanded = $state(false);

  const status = $derived(userStatus(asset.status));
  const unidentified = $derived(status === "unidentified");
  const tags = $derived(asset.tags.map((tag) => chipOf(tag, traits)));
  const shown = $derived(expanded ? tags : tags.slice(0, VISIBLE_CHIPS));
  const imported = $derived(importedState(asset, project));
  const size = $derived(formatBytes(asset.size_bytes));
  const anchor = (name?: string) => (anchored ? name : undefined);
</script>

<ItemCard
  {status}
  title={asset.name}
  {selected}
  {tabStop}
  pin={{ topStart: picking || picked, bottomEnd: asset.staged }}
  {onopen}
  oncontextmenu={onmenu}
  aria-selected={selected || picked}
  aria-rowindex={row}
  aria-colindex={col}
  data-id={asset.id}
  data-anchor={anchor("card")}
  titleAttrs={{ "data-anchor": anchor("card.name") }}
  bodyAttrs={{ "data-anchor": anchor("card.body") }}
  metaAttrs={{ "data-anchor": anchor("card.meta") }}
>
  {#snippet media()}
    {#if asset.media_type === "audio"}
      <Thumb data-anchor={anchor("card.thumb")}>
        <IconButton variant="round" icon="play" iconSize="11" label="Play {asset.name}" tabindex={-1} onclick={() => onaction?.("play")} />
        <MiniWave unknown={unidentified} />
      </Thumb>
    {:else}
      <Thumb src={asset.preview_url} unknown={unidentified} data-anchor={anchor("card.thumb")} />
    {/if}
  {/snippet}

  {#snippet topStart()}
    <Checkbox
      checked={picked}
      onchange={() => onpick?.()}
      label="Select {asset.name} for bulk actions"
      hideLabel
      tabindex={-1}
      boxAttrs={{ "data-anchor": anchor("card.pick") }}
    />
  {/snippet}

  {#snippet topEnd()}
    <IconButton
      variant="tile"
      icon="nav-favorites"
      iconSize="10"
      label={asset.favorite ? "Remove from Favorites" : "Add to Favorites"}
      pressed={asset.favorite}
      tabindex={-1}
      onclick={() => onaction?.("favorite")}
      data-anchor={anchor("card.favorite")}
    />
    <StatusGem {status} data-anchor={anchor("card.status")} />
  {/snippet}

  {#snippet bottomStart()}
    <SizeTier tier={sizeTier(asset.size_bytes, thresholds)} {size} compact data-anchor={anchor("card.size")} />
    {#if imported}
      <ImportedBadge label={imported.label} tone={imported.tone} data-anchor={anchor("card.imported")} />
    {/if}
  {/snippet}

  {#snippet bottomEnd()}
    <Button
      size="xs"
      onMedia
      accent="arcane"
      icon="action-fullsize"
      iconSize="10"
      iconBox="8"
      aria-label="Quick Look"
      title="Full-size preview"
      tabindex={-1}
      onclick={() => onaction?.("quick-look")}
      data-anchor={anchor("card.quicklook")}
    />
    <Button size="xs" onMedia active={asset.staged} aria-pressed={asset.staged} tabindex={-1} onclick={() => onaction?.("stage")} data-anchor={anchor("card.stage")}>
      {asset.staged ? "✓ staged" : "+ stage"}
    </Button>
  {/snippet}

  {#snippet mark()}
    <TypeMark tone={typeTone(asset.media_type)} label={typeLabel(asset.media_type)} data-anchor={anchor("card.type")} />
  {/snippet}

  {#snippet meta()}
    {#if asset.package}
      {@const pack = asset.package}
      <TextLink tabindex={-1} title="Open pack stats" onclick={() => onpack?.(pack)}>{pack.name}</TextLink>{" · "}
    {/if}{statLine(asset)}
  {/snippet}

  {#snippet chips()}
    {#if unidentified}
      <Chip label="???" tone="unknown" />
    {:else}
      {#each shown as chip, i (chip.name)}
        <TraitChip
          name={chip.name}
          source={chip.source}
          confidence={chip.confidence}
          color={chip.color}
          icon={chip.icon}
          tabindex={-1}
          title="Search for {chip.name}"
          onclick={() => ontag?.(chip.name)}
          data-anchor={anchor(i === 0 ? "card.chip.lead" : i === 1 ? "card.chip" : undefined)}
        />
      {/each}
      {#if tags.length > VISIBLE_CHIPS}
        <Chip
          label={expanded ? "− less" : `+${tags.length - VISIBLE_CHIPS}`}
          tone="neutral"
          tabindex={-1}
          aria-expanded={expanded}
          aria-label={expanded ? "Show fewer traits" : `Show ${tags.length - VISIBLE_CHIPS} more traits`}
          onclick={() => (expanded = !expanded)}
          data-anchor={anchor("card.chip.more")}
        />
      {/if}
    {/if}
  {/snippet}
</ItemCard>
