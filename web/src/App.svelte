<script lang="ts">
  // The shell (app/Shell.svelte) takes its data as props and is shown with fixture data at
  // /gallery.html#shell; it mounts here once P6b feeds it from /v1. Until then the page says what
  // it is rather than pretending to be the app.
</script>

<main class="placeholder">
  <h1>Lodestash</h1>
  <p>The web UI is being rebuilt (plan P6). The component gallery is at <a href="/gallery.html">/gallery.html</a>.</p>
</main>

<style>
  .placeholder {
    padding: var(--space-30);
  }

  h1 {
    font-family: var(--ff-display);
    font-size: var(--fs-24);
    font-weight: var(--fw-700);
    letter-spacing: var(--track-1);
    color: var(--ls-gold);
    margin: 0 0 var(--space-10);
  }

  p {
    color: var(--ls-text-muted);
    margin: 0;
  }
</style>
