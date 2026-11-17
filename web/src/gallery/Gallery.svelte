<script lang="ts">
  import Icon from "../ui/Icon.svelte";
  import { ICON_NAMES } from "../ui/icons.generated";
</script>

<!--
  The gallery renders a specimen of every primitive. Specimens that reproduce a design-contract
  anchor carry `data-anchor="<name>"` and the same text the prototype shows, so the gate
  (web/e2e/gate.mjs --gallery) can measure them against docs/design/contract.json (plan P6 Task 6.4).
-->
<main class="gallery">
  <h1>Gallery</h1>

  <section>
    <h2>Icons</h2>
    <ul class="icons">
      {#each ICON_NAMES as name (name)}
        <li title={name}><Icon {name} /></li>
      {/each}
    </ul>
  </section>
</main>

<style>
  .gallery {
    padding: var(--space-20);
  }

  h1 {
    font-family: var(--ff-display);
    font-size: var(--fs-20);
    color: var(--ls-gold);
    margin: 0 0 var(--space-14);
  }

  h2 {
    font-size: var(--fs-10);
    font-weight: var(--fw-600);
    letter-spacing: var(--track-1_2);
    color: var(--ls-text-dim);
    text-transform: uppercase;
    margin: 0 0 var(--space-10);
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
