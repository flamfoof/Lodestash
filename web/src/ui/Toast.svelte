<script lang="ts">
  import Icon from "./Icon.svelte";

  /**
   * One toast: a raised note with a 3 px accent rail in its intent's colour (success, warning,
   * danger, arcane for AI, info). Announced politely (`role="status"`); a danger toast is an alert.
   * `action` is the one thing it offers ("Undo").
   */
  let {
    message,
    tone = "info",
    action,
    ondismiss,
  }: {
    message: string;
    tone?: "success" | "warning" | "danger" | "arcane" | "info" | "gold" | "muted";
    action?: { label: string; onclick: () => void };
    ondismiss?: () => void;
  } = $props();
</script>

<div class="toast {tone}" role={tone === "danger" ? "alert" : "status"}>
  <span class="msg">{message}</span>
  {#if action}<button type="button" class="action" onclick={action.onclick}>{action.label}</button>{/if}
  {#if ondismiss}<button type="button" class="dismiss" aria-label="Dismiss" onclick={ondismiss}><Icon name="action-clear" size="11" /></button>{/if}
</div>

<style>
  .toast {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    max-width: var(--toast-max-w);
    padding: var(--space-8) var(--space-14);
    background: var(--ls-overlay);
    border: 1px solid var(--ls-border);
    border-left: var(--bw-3) solid var(--toast-accent);
    border-radius: var(--r-6);
    box-shadow: var(--shadow-8-24-a45);
    font-size: var(--fs-12);
    color: var(--ls-text);
    pointer-events: auto;
    animation: rise var(--dur-160) ease;
  }

  .success {
    --toast-accent: var(--ls-success);
  }
  .warning {
    --toast-accent: var(--ls-warning);
  }
  .danger {
    --toast-accent: var(--ls-danger);
  }
  .arcane {
    --toast-accent: var(--ls-arcane);
  }
  .info {
    --toast-accent: var(--ls-info);
  }
  .gold {
    --toast-accent: var(--ls-gold);
  }
  .muted {
    --toast-accent: var(--ls-text-muted);
  }

  .msg {
    flex: 1;
  }

  .action {
    color: var(--ls-gold);
    font-weight: var(--fw-600);
  }

  .action:hover {
    color: var(--ls-gold-bright);
  }

  .dismiss {
    display: flex;
    color: var(--ls-text-dim);
  }

  .dismiss:hover {
    color: var(--ls-text);
  }

  @keyframes rise {
    from {
      transform: translateY(var(--space-6));
      opacity: 0;
    }
  }
</style>
