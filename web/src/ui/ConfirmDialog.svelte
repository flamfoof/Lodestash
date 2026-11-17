<script lang="ts">
  import Button from "./Button.svelte";
  import Dialog from "./Dialog.svelte";

  /**
   * Ask before acting. `destructive` draws the confirm button in the danger accent and makes the
   * **safe** action (cancel) the one that has focus when the dialog opens, so Enter does not
   * destroy anything by reflex (DESIGN-GUIDELINES §7: destructive confirms are modals).
   */
  let {
    open = $bindable(false),
    title,
    message,
    confirmLabel = "Confirm",
    cancelLabel = "Cancel",
    destructive = false,
    onconfirm,
    oncancel,
  }: {
    open?: boolean;
    title: string;
    message: string;
    confirmLabel?: string;
    cancelLabel?: string;
    destructive?: boolean;
    onconfirm: () => void;
    oncancel?: () => void;
  } = $props();

  function confirm() {
    open = false;
    onconfirm();
  }

  function cancel() {
    open = false;
    oncancel?.();
  }
</script>

<Dialog bind:open {title} description={message} onclose={() => oncancel?.()}>
  {#snippet footer()}
    <Button variant="secondary" data-autofocus={destructive ? "" : undefined} onclick={cancel}>{cancelLabel}</Button>
    <Button variant={destructive ? "danger" : "primary"} data-autofocus={destructive ? undefined : ""} onclick={confirm}>
      {confirmLabel}
    </Button>
  {/snippet}
</Dialog>
