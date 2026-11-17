<script lang="ts">
  import Menu, { type MenuEntry } from "./Menu.svelte";

  /**
   * The right-click menu: a `Menu` opened at the pointer. Pass the `contextmenu` event as `event`
   * (its default is prevented here) — or `null` to close. Opened from the keyboard (the Menu key,
   * Shift+F10), pass the focused element as `anchor` instead and it opens below it.
   */
  let {
    event = $bindable(null),
    anchor = null,
    items,
    label,
    onselect,
    onclose,
  }: {
    event?: MouseEvent | null;
    anchor?: HTMLElement | null;
    items: MenuEntry[];
    label: string;
    onselect: (id: string) => void;
    onclose?: () => void;
  } = $props();

  let open = $state(false);
  let point = $state<{ x: number; y: number } | null>(null);

  $effect(() => {
    if (event) {
      event.preventDefault();
      point = { x: event.clientX, y: event.clientY };
      open = true;
    } else if (anchor) {
      point = null;
      open = true;
    } else open = false;
  });
</script>

<Menu
  bind:open
  {items}
  {label}
  anchor={event ? null : anchor}
  {point}
  {onselect}
  onclose={() => {
    event = null;
    onclose?.();
  }}
/>
