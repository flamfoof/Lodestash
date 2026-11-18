<script lang="ts" module>
  /** Where a linked file was found (UI-SPEC §13.6). */
  export type LinkRowState = "in_pack" | "in_library" | "missing" | "ignored";
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * One linked file in the inspector: a kind mark (◇ material, ▦ texture — a texture is indented
   * under its material), the file's name in mono, and where it was found:
   * ✓ in pack · ⤷ in library · ✕ missing · ⊘ ignored. The state is a glyph and words as well as a
   * colour. With `onclick` the row is a `<button>` (a file in another pack opens that asset).
   * `anchors` marks the row, its name and its state for the contract gate.
   */
  let {
    name,
    kind,
    state,
    indent = false,
    title,
    onclick,
    anchors = {},
  }: {
    name: string;
    kind: "material" | "texture" | "other";
    state: LinkRowState;
    indent?: boolean;
    title?: string;
    onclick?: (event: MouseEvent) => void;
    anchors?: { row?: string; name?: string; state?: string };
  } = $props();

  const KIND: Record<typeof kind, IconName> = { material: "type-material", texture: "type-texture", other: "dependency" };
  const STATE: Record<LinkRowState, { icon: IconName; label: string }> = {
    in_pack: { icon: "status-identified", label: "in pack" },
    in_library: { icon: "dependency", label: "in library" },
    missing: { icon: "action-clear", label: "missing" },
    ignored: { icon: "action-exclude", label: "ignored" },
  };
</script>

{#snippet content()}
  <span class="kind {kind}" class:indent><Icon name={KIND[kind]} size="10" box="10" /></span>
  <span class="name" data-anchor={anchors.name}>{name}</span>
  <span class="state" data-anchor={anchors.state}><Icon name={STATE[state].icon} size="10" box="8" /> {STATE[state].label}</span>
{/snippet}

{#if onclick}
  <button type="button" class="link {state}" {title} {onclick} data-anchor={anchors.row}>{@render content()}</button>
{:else}
  <div class="link {state}" {title} data-anchor={anchors.row}>{@render content()}</div>
{/if}

<style>
  .link {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    width: 100%;
    padding: var(--space-4) var(--space-9);
    background: var(--ls-surface-raised);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-6);
    font-size: var(--fs-11);
    text-align: left;
    transition: border-color var(--dur-120) ease;
  }

  div.link {
    cursor: default;
  }

  button.link:hover {
    border-color: var(--ls-arcane);
  }

  .kind {
    flex: none;
    font-size: var(--fs-10);
  }

  .material {
    color: var(--ls-type-material);
  }

  .texture,
  .other {
    color: var(--ls-type-image);
  }

  .indent {
    padding-left: var(--space-12);
  }

  .name {
    flex: 1;
    min-width: 0;
    font-family: var(--ff-mono);
    font-size: var(--fs-10);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }

  .state {
    font-size: var(--fs-10);
    white-space: nowrap;
  }

  .in_pack .state {
    color: var(--ls-success);
  }

  .in_library .state {
    color: var(--ls-arcane);
  }

  .missing {
    border-color: var(--ls-danger-a35);
  }

  .missing .name {
    color: var(--ls-status-unidentified);
  }

  .missing .state {
    color: var(--ls-danger);
  }

  button.missing:hover {
    border-color: var(--ls-warning);
  }

  .ignored .name,
  .ignored .state {
    color: var(--ls-text-dim);
  }
</style>
