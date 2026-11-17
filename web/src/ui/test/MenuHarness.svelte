<script lang="ts">
  // Test fixture: an opener and the menu it opens.
  import Menu, { type MenuEntry } from "../Menu.svelte";

  let { onpick = () => {}, items }: { onpick?: (id: string) => void; items: MenuEntry[] } = $props();

  let open = $state(false);
  let opener = $state<HTMLButtonElement>();
</script>

<button id="opener" bind:this={opener} onclick={() => (open = true)}>Actions</button>
<Menu bind:open {items} anchor={opener} label="Actions" onselect={onpick} />
