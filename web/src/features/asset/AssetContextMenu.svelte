<script lang="ts">
  import { ContextMenu } from "../../ui";
  import { intentFor, isActionId, menuFor, type AssetIntent } from "./actions";
  import type { AssetSummary, ProjectRef } from "./model";

  /**
   * The right-click menu of an asset (DESIGN-GUIDELINES §7 "Selection & context menu"). The same
   * menu serves one asset and many: `targets` is what it acts on — the whole pick set when the
   * asset it was opened on is in it, otherwise that asset alone (`Selection.targets`). Its items,
   * their order, what is disabled and why all come from `actions.ts`; choosing one emits the intent
   * that module defines. `at` is where it opens: the pointer's `event`, or the card (`anchor`) when
   * it was asked for from the keyboard. Null is closed.
   */
  let {
    at = $bindable(null),
    targets,
    project,
    onintent,
  }: {
    at: { event?: MouseEvent; anchor?: HTMLElement } | null;
    targets: readonly AssetSummary[];
    project: ProjectRef | null;
    onintent?: (intent: AssetIntent) => void;
  } = $props();

  const ctx = $derived({ targets, project });

  function choose(id: string) {
    if (!isActionId(id)) return;
    const intent = intentFor(id, ctx);
    if (intent) onintent?.(intent);
  }
</script>

<ContextMenu
  event={at?.event ?? null}
  anchor={at?.anchor ?? null}
  items={menuFor(ctx)}
  label={targets.length === 1 ? `Actions for ${targets[0].name}` : `Actions for ${targets.length} assets`}
  onselect={choose}
  onclose={() => (at = null)}
/>
