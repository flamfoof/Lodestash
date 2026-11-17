<script lang="ts">
  /**
   * A headline figure: the Stats screen's tile — the value large in mono (in the metric's colour),
   * its label, and a dim line of context.
   */
  let {
    value,
    label,
    sub,
    tone = "text",
  }: {
    value: string;
    label: string;
    sub?: string;
    tone?: "text" | "gold" | "success" | "warning" | "danger" | "arcane" | "model" | "sound" | "image";
  } = $props();
</script>

<div class="stat">
  <div class="value {tone}">{value}</div>
  <div class="label">{label}</div>
  {#if sub}<div class="sub">{sub}</div>{/if}
</div>

<style>
  .stat {
    padding: var(--space-14) var(--space-16);
    background: var(--ls-surface);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-10);
  }

  .value {
    font-family: var(--ff-mono);
    font-size: var(--fs-22);
  }

  .label {
    margin-top: var(--space-2);
    font-size: var(--fs-12);
    font-weight: var(--fw-600);
    color: var(--ls-text);
  }

  .sub {
    margin-top: var(--space-1);
    font-size: var(--fs-11);
    color: var(--ls-text-dim);
  }

  .text {
    color: var(--ls-text);
  }
  .gold {
    color: var(--ls-gold);
  }
  .success {
    color: var(--ls-success);
  }
  .warning {
    color: var(--ls-warning);
  }
  .danger {
    color: var(--ls-danger);
  }
  .arcane {
    color: var(--ls-arcane);
  }
  .model {
    color: var(--ls-type-model);
  }
  .sound {
    color: var(--ls-type-sound);
  }
  .image {
    color: var(--ls-type-image);
  }
</style>
