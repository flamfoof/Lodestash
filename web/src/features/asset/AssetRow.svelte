<script lang="ts">
  import { Button, Checkbox, Icon, ItemRow, SizeTier, StatusLine, TextLink, Thumb, TypeMark } from "../../ui";
  import type { AssetActionId } from "./actions";
  import type { AssetSummary, PackageRef, SizeThresholds } from "./model";
  import { formatBytes, sizeTier, statLine, typeLabel, typeTone, userStatus } from "./present";

  /**
   * One asset as a list row (UI-SPEC §2 item 5): the card's facts on one line — thumb · type ·
   * name · pack · size · stats · status · ☆ · ⤢ · stage. Composition and wiring only; every look
   * is a primitive's.
   *
   * It is the card's twin: the same props, the same events, the same rule that a button here names
   * an action (`onaction`) and never builds an intent — so the grid that lays out cards lays out
   * rows with the same selection and the same keyboard, and nothing about either is written twice.
   * A row is a container whose primary action is the name; its buttons are `tabindex="-1"`.
   *
   * The pick box sits over the thumb and shows as the card's does: on hover, with focus inside,
   * and while anything is picked. Rows carry no chips.
   */
  let {
    asset,
    selected = false,
    picked = false,
    picking = false,
    tabStop = false,
    alt = false,
    thresholds,
    anchors,
    row,
    col,
    onopen,
    onpick,
    onaction,
    onmenu,
    onpack,
  }: {
    asset: AssetSummary;
    /** The lead: the gold ring, what the inspector describes. */
    selected?: boolean;
    /** In the pick set. */
    picked?: boolean;
    /** Anything is picked: the pick boxes stay visible. */
    picking?: boolean;
    tabStop?: boolean;
    /** Every second row (the zebra). */
    alt?: boolean;
    thresholds: SizeThresholds;
    /**
     * What the contract gate marks on this row: every part of the `first`, the row itself on the
     * `alt` one, the name on the first `unknown` one.
     */
    anchors?: "first" | "alt" | "unknown";
    /** The row's place in the grid, 1-based: it says so itself (AssetGrid explains why). */
    row?: number;
    col?: number;
    onopen?: (event: MouseEvent) => void;
    onpick?: () => void;
    onaction?: (id: AssetActionId) => void;
    onmenu?: (event: MouseEvent) => void;
    onpack?: (pack: PackageRef) => void;
  } = $props();

  const status = $derived(userStatus(asset.status));
  const unidentified = $derived(status === "unidentified");
  const sound = $derived(asset.media_type === "audio");
  const anchor = (name: string) => (anchors === "first" ? name : undefined);
</script>

<ItemRow
  {status}
  title={asset.name}
  {selected}
  {alt}
  {tabStop}
  pin={picking || picked}
  {onopen}
  oncontextmenu={onmenu}
  aria-selected={selected || picked}
  aria-rowindex={row}
  aria-colindex={col}
  data-id={asset.id}
  data-anchor={anchors === "alt" ? "row.alt" : anchor("row")}
  titleAttrs={{ "data-anchor": anchors === "unknown" ? "row.name.unknown" : anchor("row.name") }}
  packAttrs={{ "data-anchor": anchor("row.pack") }}
  statsAttrs={{ "data-anchor": anchor("row.stats") }}
>
  {#snippet media()}
    <Thumb size="row" src={asset.preview_url} unknown={unidentified} data-anchor={anchor("row.thumb")}>
      {#if sound && !unidentified}<Icon name="type-sound" size="12" />{/if}
    </Thumb>
  {/snippet}

  {#snippet pick()}
    <Checkbox checked={picked} onchange={() => onpick?.()} label="Select {asset.name} for bulk actions" hideLabel tabindex={-1} />
  {/snippet}

  {#snippet mark()}
    <TypeMark tone={typeTone(asset.media_type)} label={typeLabel(asset.media_type)} data-anchor={anchor("row.type")} />
  {/snippet}

  {#snippet pack()}
    {#if asset.package}
      {@const pack = asset.package}
      <TextLink tabindex={-1} title="Open pack stats" onclick={() => onpack?.(pack)}>{pack.name}</TextLink>
    {/if}
  {/snippet}

  {#snippet size()}
    <SizeTier variant="text" tier={sizeTier(asset.size_bytes, thresholds)} size={formatBytes(asset.size_bytes)} data-anchor={anchor("row.size")} />
  {/snippet}

  {#snippet stats()}{statLine(asset)}{/snippet}

  {#snippet state()}
    <StatusLine {status} inline data-anchor={anchor("row.status")} />
  {/snippet}

  {#snippet actions()}
    <Button
      variant="favorite"
      size="xs"
      icon="nav-favorites"
      iconBox="12"
      active={asset.favorite}
      aria-pressed={asset.favorite}
      aria-label={asset.favorite ? "Remove from Favorites" : "Add to Favorites"}
      title="Favorite"
      tabindex={-1}
      onclick={() => onaction?.("favorite")}
      data-anchor={anchor("row.favorite")}
    />
    <Button
      size="xs"
      accent="arcane"
      icon="action-fullsize"
      iconSize="10"
      iconBox="8"
      aria-label="Quick Look"
      title="Full-size preview"
      tabindex={-1}
      onclick={() => onaction?.("quick-look")}
      data-anchor={anchor("row.quicklook")}
    />
    <Button size="xs" accent="gold" active={asset.staged} aria-pressed={asset.staged} tabindex={-1} onclick={() => onaction?.("stage")} data-anchor={anchor("row.stage")}>
      {asset.staged ? "✓ staged" : "+ stage"}
    </Button>
  {/snippet}
</ItemRow>
