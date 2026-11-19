<script lang="ts">
  import { Button, Icon, Input, Text, TextLink, Toolbar } from "../../ui";
  import { intentFor, whyNot, type ActionArgs, type AssetActionId, type AssetIntent } from "../asset/actions";
  import type { AssetSummary, ProjectRef } from "../asset/model";
  import { userStatus } from "../asset/present";

  /**
   * The bulk bar (UI-SPEC §2 item 4): while anything is picked, a strip under the toolbar that
   * says how many and offers what can be done to all of them — add or remove a tag by name, stage
   * the lot, or let the picks go.
   *
   * It acts on `targets` — the pick set, resolved to assets by whoever owns the page — and only
   * through `actions.ts`: each button asks for the intent and reports it. A tag edit leaves the
   * Unidentified out (the intent carries only the assets it applies to); "Stage all" is the one
   * stage toggle, so on a set that is staged already it reads and does "Unstage all".
   *
   * The tag name is the bar's own draft: it is nobody else's business until a button sends it.
   * A button that cannot run says why beside itself and puts focus in the field — it is never
   * greyed out for an empty name, because the name is typed *after* the picks are made.
   */
  let {
    targets,
    project,
    onintent,
    onclear,
  }: {
    /** The pick set, in the page's order. With none the bar is not rendered. */
    targets: readonly AssetSummary[];
    project: ProjectRef | null;
    onintent?: (intent: AssetIntent) => void;
    /** Empty the pick set (the lead stays). */
    onclear?: () => void;
  } = $props();

  let tag = $state("");
  let reason = $state<string | null>(null);
  let field = $state<HTMLElement>();

  const ctx = $derived({ targets, project });
  const staged = $derived(targets.length > 0 && targets.every((a) => a.staged));
  const skipped = $derived(targets.filter((a) => userStatus(a.status) === "unidentified").length);
  const skipNote = $derived(skipped > 0 ? ` — ${skipped} Unidentified ${skipped === 1 ? "is" : "are"} skipped` : "");

  function run(id: AssetActionId, args: ActionArgs = {}) {
    reason = whyNot(id, ctx, args) ?? null;
    if (reason !== null) {
      field?.querySelector("input")?.focus();
      return;
    }
    const intent = intentFor(id, ctx, args);
    if (intent) onintent?.(intent);
  }
</script>

{#if targets.length > 0}
  <Toolbar tone="selection" label="Bulk actions" data-anchor="bulk">
    <Text variant="selection" aria-live="polite" data-anchor="bulk.count">{targets.length} selected</Text>
    <span class="tag" bind:this={field}>
      <Input
        label="Tag name"
        hideLabel
        size="bar"
        fill
        placeholder="tag name…"
        bind:value={tag}
        oninput={() => (reason = null)}
        onkeydown={(event) => event.key === "Enter" && run("tag-add", { tag })}
        data-anchor="bulk.tag"
      />
    </span>
    <Button variant="quiet-gold" size="bar" title="Add this tag to the selection{skipNote}" onclick={() => run("tag-add", { tag })} data-anchor="bulk.add">+ Add tag</Button>
    <Button size="bar" title="Remove this tag from the selection{skipNote}" onclick={() => run("tag-remove", { tag })} data-anchor="bulk.remove">− Remove tag</Button>
    <Button variant="quiet-gold" size="bar" icon="action-stage" iconBox="8" onclick={() => run("stage")} data-anchor="bulk.stage">
      {staged ? "Unstage all" : "Stage all"}
    </Button>
    {#if reason}<Text variant="warning" role="status">{reason}</Text>{/if}
    <span class="clear">
      <TextLink tone="dim" hover="text" size="12" title="Clear the selection" onclick={() => onclear?.()} data-anchor="bulk.clear">
        <Icon name="action-clear" size="10" box="9" /> Clear
      </TextLink>
    </span>
  </Toolbar>
{/if}

<style>
  /* Layout only: the field's width, and ✕ Clear at the far end. */
  .tag {
    display: flex;
    width: var(--bulk-tag-w);
    flex-shrink: 0;
  }

  .clear {
    display: flex;
    flex-shrink: 0;
    margin-left: auto;
    white-space: nowrap;
  }
</style>
