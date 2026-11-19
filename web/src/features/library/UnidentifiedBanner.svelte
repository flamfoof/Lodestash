<script lang="ts">
  import { Banner } from "../../ui";

  /**
   * The Unidentified nudge (UI-SPEC §2.1, FEATURES §1.5): while any asset is Unidentified, a slim
   * band says how many and why it matters, with the way to Identify mode. It can be dismissed for
   * the session; whoever owns the session remembers that (`ondismiss`).
   */
  let {
    count,
    onidentify,
    ondismiss,
  }: {
    count: number;
    onidentify?: () => void;
    ondismiss?: () => void;
  } = $props();
</script>

{#if count > 0}
  <Banner
    figure={count}
    action={{ label: "Open Identify mode →", onclick: () => onidentify?.() }}
    actionAttrs={{ "data-anchor": "library.banner.action" }}
    {ondismiss}
    data-anchor="library.banner"
  >
    {count === 1 ? "asset is" : "assets are"} Unidentified — invisible to AI search until appraised.
  </Banner>
{/if}
