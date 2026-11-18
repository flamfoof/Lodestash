<script lang="ts">
  // Test fixture: the grid between two buttons, so a test can see what Tab would and would not reach.
  import { PROJECT, SIZE_THRESHOLDS, TRAITS } from "../../../fixtures/library";
  import type { Selection } from "../../../state/selection.svelte";
  import type { AssetIntent } from "../actions";
  import AssetGrid from "../AssetGrid.svelte";
  import type { AssetSummary, PackageRef } from "../model";

  let {
    assets,
    selection,
    columns = 5,
    onintent = () => {},
    onmenu = () => {},
    onpack = () => {},
    ontag = () => {},
  }: {
    assets: AssetSummary[];
    selection: Selection;
    columns?: number;
    onintent?: (intent: AssetIntent) => void;
    onmenu?: (asset: AssetSummary, at: { event?: MouseEvent; anchor?: HTMLElement }) => void;
    onpack?: (pack: PackageRef) => void;
    ontag?: (value: string) => void;
  } = $props();
</script>

<button id="before">before</button>
<AssetGrid {assets} {selection} project={PROJECT} traits={TRAITS} thresholds={SIZE_THRESHOLDS} {columns} {onintent} {onmenu} {onpack} {ontag} />
<button id="after">after</button>
