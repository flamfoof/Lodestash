<script lang="ts">
  /**
   * First-run onboarding (UI-SPEC §1) — two steps over the **live library**, never a blank page.
   *
   * This was a static explainer until 2026-08-28: three numbered sentences and a "Get started".
   * It described the app instead of setting it up, and its third sentence promised you could
   * "pull the right asset into Unity in one step", which nothing in the app can do yet.
   *
   * **Step 1** shows what Lodestash has actually found — the sources already indexed, and the
   * Unity Asset Store cache detected on disk but not yet added — and offers the three actions
   * §1 specifies. **Step 2** runs the scan and reports it.
   *
   * Two places this narrows the spec, both because of what the backend can honestly report:
   *
   * - §1 wants **"M new files on disk aren't indexed yet"** in the returning-user banner. Counting
   *   them means walking every source root, which costs the same as the scan it is advertising —
   *   so the banner says what it knows (assets indexed, when last scanned) and the button offers
   *   the scan without pretending to know its size.
   * - §1 wants a progress bar and **"a mono log line per pipeline stage"**. `add_source` is
   *   synchronous and reports nothing mid-crawl (its own docs: *"a fake bar that jumps 0 → 100 is
   *   worse than an honest spinner"*). So the bar measures **sources completed**, which is real,
   *   and each log line is a source that genuinely finished with the count it returned.
   *
   * Both are written into UI-SPEC §1 in the same change.
   */
  import Icon from "./Icon.svelte";
  import {
    getSettings,
    setSettings,
    libraryStatus,
    addSource,
    addSourceDialog,
    unityCacheCandidates,
    type LibraryStatus,
    type CacheCandidate,
  } from "./api";
  import { view } from "./nav.svelte";

  let { onclose, onindexed }: { onclose: () => void; onindexed?: () => void } = $props();

  let skip = $state(false);
  let step = $state<1 | 2>(1);
  let status = $state<LibraryStatus | null>(null);
  let candidates = $state<CacheCandidate[]>([]);
  let busy = $state(false);
  let error = $state<string | null>(null);

  /** Scan progress — sources finished / to do, and one line per source that completed. */
  let scanTotal = $state(0);
  let scanDone = $state(0);
  let log = $state<string[]>([]);
  let indexedThisRun = $state(0);
  let finished = $state(false);

  let sources = $derived(status?.sources ?? []);
  let hasIndex = $derived((status?.total_assets ?? 0) > 0);
  /** The most recent completed crawl across every source — the banner's "last scanned". */
  let lastScanned = $derived(
    sources.reduce<number | null>(
      (best, s) => (s.last_scanned_at && (!best || s.last_scanned_at > best) ? s.last_scanned_at : best),
      null,
    ),
  );

  async function refresh() {
    try {
      status = await libraryStatus();
    } catch {
      /* the panel still works; it just lists nothing */
    }
    try {
      candidates = await unityCacheCandidates();
    } catch {
      candidates = [];
    }
  }
  refresh();

  const fmtWhen = (unix: number | null) => {
    if (!unix) return "never";
    const mins = Math.max(0, Math.floor(Date.now() / 1000 - unix) / 60);
    if (mins < 1) return "just now";
    if (mins < 60) return `${Math.floor(mins)} min ago`;
    const hrs = mins / 60;
    if (hrs < 24) return `${Math.floor(hrs)} h ago`;
    return `${Math.floor(hrs / 24)} d ago`;
  };
  const n = (v: number) => v.toLocaleString();

  async function done() {
    if (skip) {
      try {
        const s = await getSettings();
        await setSettings({ ...s, skip_onboarding: true });
      } catch {
        // Non-fatal: the panel still closes; the flag just does not stick this time.
      }
    }
    onclose();
  }

  /** `+ Add another folder…` — the OS picker, then the same import path as dropping a folder. */
  async function addFolder() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await addSourceDialog();
      await refresh();
      onindexed?.();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  /** Add one detected cache root. Same crawl, already pointed at the right place. */
  async function addCandidate(path: string) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await addSource(path);
      await refresh();
      onindexed?.();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  /**
   * Step 2: re-crawl every available source, one at a time.
   *
   * Sequential on purpose — `add_source` is a synchronous disk walk, and running several at once
   * would contend on the same disk while making the progress meaningless. A source that fails is
   * logged and the run continues: one unplugged drive must not stop the others.
   */
  async function scan() {
    step = 2;
    finished = false;
    log = [];
    indexedThisRun = 0;
    const targets = sources.filter((s) => s.state !== "unavailable");
    scanTotal = targets.length;
    scanDone = 0;
    busy = true;
    for (const s of targets) {
      try {
        const r = await addSource(s.root_path);
        indexedThisRun += r.changed;
        log = [...log, `✓ ${s.name} — ${n(r.changed)} new or changed`];
      } catch (e) {
        log = [...log, `✕ ${s.name} — ${String(e)}`];
      }
      scanDone += 1;
    }
    for (const s of sources.filter((x) => x.state === "unavailable")) {
      log = [...log, `– ${s.name} — skipped, the root is not reachable`];
    }
    busy = false;
    finished = true;
    await refresh();
    onindexed?.();
  }

  function watchProgress() {
    view.go("identify");
    done();
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={() => !busy && onclose()} data-testid="onboarding">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="sheet"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    aria-label="Welcome to Lodestash"
  >
    <header>
      <span class="gem" aria-hidden="true"><Icon name="ls-logo" size={24} /></span>
      <h1>{step === 1 ? "Point it at your hoard." : "Scanning."}</h1>
      <span class="stepno num">step {step}/2</span>
      <button class="close" onclick={onclose} aria-label="Close" disabled={busy}>✕</button>
    </header>

    {#if step === 1}
      <p class="lead">
        Lodestash indexes everything you own — packs, folders, loose files — so you can find the
        one asset you need. <strong>Nothing leaves this machine.</strong>
      </p>

      {#if hasIndex}
        <!-- Returning user (§1). It states what it knows: how much is indexed and when it last
             ran. It does **not** claim a count of un-indexed files — finding that out costs a
             full walk of every root, which is the scan it would be advertising. -->
        <p class="banner" data-testid="onboarding-existing">
          <span class="dot" aria-hidden="true">◆</span>
          Existing index found — <b class="num">{n(status?.total_assets ?? 0)}</b> assets across
          <b class="num">{n(sources.length)}</b>
          {sources.length === 1 ? "source" : "sources"}, last scanned {fmtWhen(lastScanned)}.
        </p>
      {/if}

      <ul class="sources" data-testid="onboarding-sources">
        <!-- Detected-but-not-added first, per §1: the Asset Store cache is the one source most
             users have and none of them added by hand. -->
        {#each candidates as c (c.path)}
          <li class="src detected">
            <span class="diamond arcane" aria-hidden="true">◆</span>
            <span class="who">
              <span class="nm">Unity Asset Store cache</span>
              <span class="path num" title={c.path}>{c.path}</span>
            </span>
            <span class="count num">{n(c.packs)} {c.packs === 1 ? "pack" : "packs"}</span>
            <span class="pill arcane">auto-detected</span>
            <button
              class="add"
              onclick={() => addCandidate(c.path)}
              disabled={busy}
              data-testid="onboarding-add-cache">Add</button
            >
          </li>
        {/each}

        {#each sources as s (s.id)}
          <li class="src" class:gone={s.state === "unavailable"}>
            <span
              class="diamond"
              class:red={s.state === "unavailable"}
              aria-hidden="true">{s.state === "unavailable" ? "✕" : "◆"}</span
            >
            <span class="who">
              <span class="nm">{s.name}</span>
              <span class="path num" title={s.root_path}>{s.root_path}</span>
            </span>
            <span class="count num">{n(s.asset_count)} assets</span>
            <span class="pill" class:arcane={s.kind !== "folder"}>
              {s.kind === "folder" ? "folder" : "auto-detected"}
            </span>
          </li>
        {/each}

        {#if sources.length === 0 && candidates.length === 0}
          <li class="none">
            Nothing indexed yet. Add a folder below — or drop one anywhere on the window.
          </li>
        {/if}
      </ul>

      {#if candidates.length > 0}
        <p class="caption">Unity cache auto-detected</p>
      {/if}

      {#if error}<p class="err" data-testid="onboarding-error">{error}</p>{/if}

      <footer>
        <label class="skip">
          <input type="checkbox" bind:checked={skip} data-testid="skip-onboarding" />
          Don't show this next time
        </label>
        <button class="ghost" onclick={addFolder} disabled={busy} data-testid="onboarding-add"
          >+ Add another folder…</button
        >
        {#if sources.length > 0}
          <!-- The sprite, not a text glyph: §1's ⌕ is not in the bundled Inter and renders as
               tofu, exactly as §15's ◉ did. The fonts ship with the app for the offline
               invariant, so "use a font that has it" is not on the table — the icon set is. -->
          <button class="arcane-btn" onclick={scan} disabled={busy} data-testid="onboarding-scan">
            <Icon name="ls-search" size={16} /> Scan sources
          </button>
        {/if}
        <button class="go" onclick={done} disabled={busy} data-testid="onboarding-go"
          >Open library →</button
        >
      </footer>
    {:else}
      <p class="lead">
        Only new and changed files are scanned — the existing index is untouched. Non-content junk
        (scripts, DLLs, editor tools) is hidden automatically.
      </p>

      <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax={scanTotal} aria-valuenow={scanDone}>
        <span class="fill" style="--pct: {scanTotal ? (scanDone / scanTotal) * 100 : 0}%"></span>
      </div>
      <p class="caption num" data-testid="onboarding-progress">
        {n(scanDone)} / {n(scanTotal)} sources
      </p>

      <ul class="log num" data-testid="onboarding-log">
        {#each log as line, i (i)}
          <li>{line}</li>
        {/each}
      </ul>

      {#if finished}
        <!-- ADR-0008: appraisal is a background job, so the honest sentence is that it has already
             started. The pre-ADR copy asked a new user to choose between two things they had no
             way to evaluate. Assets *do* still arrive Unidentified — softening that would hide why
             the first AI searches come back thin. -->
        <p class="callout" data-testid="onboarding-done">
          ✦ {indexedThisRun > 0
            ? `${n(indexedThisRun)} new or changed ${indexedThisRun === 1 ? "asset" : "assets"} indexed.`
            : "Nothing new on disk — the index was already current."}
          Appraisal runs in the background, whatever you are looking at. Assets start
          <b>Unidentified</b> and become searchable as it goes; you do not have to wait here.
        </p>
      {/if}

      <footer>
        <button class="ghost" onclick={() => (step = 1)} disabled={busy}>← Back</button>
        {#if finished}
          <button class="arcane-btn" onclick={watchProgress} data-testid="onboarding-watch"
            >Watch progress</button
          >
        {/if}
        <button class="go" onclick={done} disabled={busy} data-testid="onboarding-go"
          >Open library →</button
        >
      </footer>
    {/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
    display: grid;
    place-items: center;
    background: rgba(10, 12, 17, 0.6);
    backdrop-filter: blur(3px);
  }
  .sheet {
    width: min(640px, 92vw);
    max-height: 88vh;
    overflow: auto;
    background: var(--ls-surface);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-lg);
    padding: var(--sp-5);
    display: flex;
    flex-direction: column;
    gap: var(--sp-4);
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.55);
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .gem {
    width: 34px;
    height: 34px;
    flex: none;
    display: grid;
    place-items: center;
    border: 2px solid var(--ls-gold);
    border-radius: 10px;
    transform: rotate(45deg);
    color: var(--ls-gold);
  }
  .gem :global(svg) {
    transform: rotate(-45deg);
  }
  h1 {
    flex: 1;
    margin: 0;
    font-family: var(--ff-display);
    font-size: var(--fs-xl);
    font-weight: 700;
    letter-spacing: 0.02em;
  }
  .stepno {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .close {
    background: none;
    border: none;
    color: var(--ls-text-dim);
    font: inherit;
    cursor: pointer;
  }
  .close:hover {
    color: var(--ls-text);
  }
  .close:disabled {
    color: var(--ls-border);
    cursor: default;
  }
  .lead {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    line-height: 1.6;
  }

  /* Returning-user banner (§1) — verdant, informational, never a warning. */
  .banner {
    display: flex;
    align-items: baseline;
    gap: var(--sp-2);
    margin: 0;
    padding: var(--sp-2) var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    background: color-mix(in srgb, var(--ls-success) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--ls-success) 35%, transparent);
    border-radius: var(--r-md);
  }
  .banner .dot {
    color: var(--ls-success);
  }
  .banner b {
    color: var(--ls-text);
  }

  .sources {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    max-height: 30vh;
    overflow: auto;
  }
  .src {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
  }
  .src.gone {
    opacity: 0.6;
  }
  /* Shape *and* colour, never colour alone (DESIGN-GUIDELINES §9): an unreachable root is a ✕. */
  .diamond {
    flex: none;
    color: var(--ls-gold);
    transform: rotate(45deg);
    font-size: var(--fs-xs);
  }
  .diamond.arcane {
    color: var(--ls-arcane);
  }
  .diamond.red {
    color: var(--ls-danger);
    transform: none;
  }
  .who {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .nm {
    color: var(--ls-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .path {
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    flex: none;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  .pill {
    flex: none;
    padding: 0 6px;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-pill);
  }
  .pill.arcane {
    color: var(--ls-arcane);
    border-color: color-mix(in srgb, var(--ls-arcane) 45%, transparent);
  }
  .add {
    flex: none;
    padding: 2px 10px;
    background: none;
    border: 1px solid color-mix(in srgb, var(--ls-arcane) 55%, transparent);
    border-radius: var(--r-sm);
    color: var(--ls-arcane);
    font: inherit;
    font-size: var(--fs-xs);
    cursor: pointer;
  }
  .add:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ls-arcane) 12%, transparent);
  }
  .none {
    padding: var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--ls-text-dim);
    border: 1px dashed var(--ls-border-strong);
    border-radius: var(--r-md);
    text-align: center;
  }
  .caption {
    margin: 0;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    text-align: right;
  }
  .err {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-danger);
  }

  /* Step 2. The bar measures sources completed — real work, coarsely counted. */
  .bar {
    height: 6px;
    background: var(--ls-overlay);
    border-radius: var(--r-pill);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    width: var(--pct, 0%);
    background: linear-gradient(90deg, var(--ls-gold), var(--ls-gold-bright));
    transition: width 180ms ease;
  }
  .log {
    list-style: none;
    margin: 0;
    padding: var(--sp-2);
    max-height: 34vh;
    overflow: auto;
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    background: var(--ls-bg);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-md);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .callout {
    margin: 0;
    padding: var(--sp-3);
    font-size: var(--fs-sm);
    line-height: 1.6;
    color: var(--ls-text-muted);
    background: color-mix(in srgb, var(--ls-arcane) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--ls-arcane) 35%, transparent);
    border-radius: var(--r-md);
  }
  .callout b {
    color: var(--ls-status-unidentified);
  }

  /* One row. `flex-wrap` plus the skip label's `margin-right: auto` dropped **Open library →**
     — the primary action — onto a line of its own *below* the two secondaries, which reads as
     the least important thing on the panel. The label shrinks instead; the buttons never do. */
  footer {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
  }
  .skip {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    cursor: pointer;
    margin-right: auto;
    min-width: 0;
  }
  footer button {
    flex: none;
    white-space: nowrap;
  }
  .skip input {
    accent-color: var(--ls-gold);
  }
  .ghost,
  .arcane-btn,
  .go {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 8px var(--sp-4);
    border-radius: var(--r-md);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: 600;
    cursor: pointer;
  }
  .ghost {
    background: none;
    border: 1px solid var(--ls-border-strong);
    color: var(--ls-text-muted);
  }
  .ghost:hover:not(:disabled) {
    border-color: var(--ls-text-muted);
    color: var(--ls-text);
  }
  .arcane-btn {
    background: none;
    border: 1px solid color-mix(in srgb, var(--ls-arcane) 55%, transparent);
    color: var(--ls-arcane);
  }
  .arcane-btn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ls-arcane) 12%, transparent);
  }
  .go {
    background: color-mix(in srgb, var(--ls-gold) 18%, transparent);
    border: 1px solid var(--ls-gold);
    color: var(--ls-gold-bright);
  }
  .go:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ls-gold) 28%, transparent);
  }
  .ghost:disabled,
  .arcane-btn:disabled,
  .go:disabled,
  .add:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
