<script lang="ts">
  // Test fixture: a destructive confirm, open from the start.
  import ConfirmDialog from "../ConfirmDialog.svelte";

  let { onconfirm = () => {}, destructive = true }: { onconfirm?: () => void; destructive?: boolean } = $props();
  let open = $state(true);
</script>

<ConfirmDialog bind:open title="Move to Trash?" message="3 assets leave the library." confirmLabel="Move to Trash" {destructive} {onconfirm} />
