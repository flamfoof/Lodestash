<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Button, { type ButtonSize } from "./Button.svelte";
  import type { IconBox, IconSize } from "./Icon.svelte";
  import type { IconName } from "./icons.generated";

  /**
   * A button that is on or off (`aria-pressed`): a view switch such as ★ Favorites. On is the
   * gold border and fill; the state is also announced, never colour alone.
   */
  let {
    pressed = $bindable(false),
    onchange,
    size = "md",
    icon,
    iconSize,
    iconBox,
    children,
    trailing,
    ...rest
  }: Omit<HTMLButtonAttributes, "onchange"> & {
    pressed?: boolean;
    onchange?: (pressed: boolean) => void;
    size?: ButtonSize;
    icon?: IconName;
    iconSize?: IconSize;
    iconBox?: IconBox;
    children?: Snippet;
    trailing?: Snippet;
  } = $props();

  function toggle() {
    pressed = !pressed;
    onchange?.(pressed);
  }
</script>

<Button
  variant="secondary"
  {size}
  {icon}
  {iconSize}
  {iconBox}
  active={pressed ? "pressed" : false}
  aria-pressed={pressed}
  onclick={toggle}
  {children}
  {trailing}
  {...rest}
/>
