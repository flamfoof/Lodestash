<script lang="ts">
  import type { Snippet } from "svelte";

  /**
   * A settings group: a card on the surface with a letter-spaced caps heading (written in capitals,
   * as the prototype writes it — not transformed), an optional line of
   * explanation, and its controls. `actions` sits right of the heading ("+ Add source"). A
   * `<section>` named by its heading.
   */
  let {
    heading,
    description,
    actions,
    children,
  }: {
    heading: string;
    description?: string;
    actions?: Snippet;
    children: Snippet;
  } = $props();

  const uid = $props.id();
</script>

<section class="card" aria-labelledby="{uid}-h">
  <div class="head">
    <h3 id="{uid}-h" class="heading">{heading}</h3>
    {#if actions}<div class="actions">{@render actions()}</div>{/if}
  </div>
  {#if description}<p class="desc">{description}</p>{/if}
  {@render children()}
</section>

<style>
  .card {
    padding: var(--space-14) var(--space-16);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-10);
    user-select: text;
  }

  .head {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    margin-bottom: var(--space-4);
  }

  .heading {
    margin: 0;
    font-size: var(--fs-10);
    font-weight: var(--fw-600);
    letter-spacing: var(--track-1_2);
    color: var(--ls-text-dim);
  }

  .actions {
    margin-left: auto;
  }

  .desc {
    margin: 0 0 var(--space-11);
    font-size: var(--fs-11);
    color: var(--ls-text-muted);
  }
</style>
