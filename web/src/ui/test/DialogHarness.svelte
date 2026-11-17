<script lang="ts">
  // Test fixture: an app root with an opener, a dialog with a nested dialog and a menu inside.
  import Dialog from "../Dialog.svelte";
  import Menu from "../Menu.svelte";

  let { onpick = () => {} }: { onpick?: (id: string) => void } = $props();

  let open = $state(false);
  let inner = $state(false);
  let menuOpen = $state(false);
  let menuButton = $state<HTMLButtonElement>();
</script>

<div id="app-root">
  <button id="opener" onclick={() => (open = true)}>Open</button>
  <button id="other">Other</button>
</div>

<Dialog bind:open title="First" description="The outer dialog.">
  <input id="a" aria-label="A" />
  <button id="b">B</button>
  <button id="nested" onclick={() => (inner = true)}>Nested</button>
  <button id="menu-button" bind:this={menuButton} onclick={() => (menuOpen = true)}>Menu</button>
  <Menu
    bind:open={menuOpen}
    anchor={menuButton}
    label="Actions"
    items={[{ id: "one", label: "One" }, { id: "two", label: "Two" }]}
    onselect={onpick}
  />
  <Dialog bind:open={inner} title="Second">
    <button id="c">C</button>
  </Dialog>
</Dialog>
