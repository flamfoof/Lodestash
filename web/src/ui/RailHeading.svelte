<script lang="ts">
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * A group heading in the rail ("STATUS", "LIBRARIES"): 10 px, letter-spaced, dim. Written in
   * capitals as the prototype writes it (not transformed).
   */
  let { text, id, ...rest }: HTMLAttributes<HTMLHeadingElement> & { text: string } = $props();
</script>

<h2 class="heading" {id} {...rest}>{text}</h2>

<style>
  .heading {
    margin: 0;
    padding: 0 var(--space-14) var(--space-6);
    font-size: var(--fs-10);
    font-weight: var(--fw-600);
    letter-spacing: var(--track-1_2);
    color: var(--ls-text-dim);
  }
</style>
