<script lang="ts">
  /**
   * First-run onboarding (FEATURES §8, UI-SPEC §1, design README §1): a centred panel over the
   * **live library** — never a blank page — pointing the user at the drop affordance. "Don't
   * show this next time" persists `skip_onboarding` to config.json; Settings › FIRST RUN mirrors
   * the same flag, so the two can never disagree.
   */
  import Icon from "./Icon.svelte";
  import { getSettings, setSettings } from "./api";

  let { onclose }: { onclose: () => void } = $props();
  let skip = $state(false);

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
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={onclose} data-testid="onboarding">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="sheet" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" tabindex="-1" aria-label="Welcome to Lodestash">
    <header>
      <span class="gem" aria-hidden="true"><Icon name="ls-logo" size={18} /></span>
      <h1>Point it at your hoard.</h1>
      <button class="close" onclick={onclose} aria-label="Close">✕</button>
    </header>

    <p class="lead">
      Lodestash catalogues the assets you already own — thousands of models, sounds and textures
      scattered across asset packs — and indexes them <strong>in place</strong>. Nothing is ever
      moved, modified, or uploaded.
    </p>

    <ol class="steps">
      <li>
        <span class="n">1</span>
        <span>Drop a folder anywhere on the window to add it as a library source.</span>
      </li>
      <li>
        <span class="n">2</span>
        <span>Assets arrive <b>Unidentified</b> (<span class="q">???</span>). Appraisal runs once
        per asset, locally — no account, no cloud.</span>
      </li>
      <li>
        <span class="n">3</span>
        <span>Search by tag or by plain description, and pull the right asset into Unity in one
        step.</span>
      </li>
    </ol>

    <div class="drop">
      <Icon name="ls-nav-packages" size={22} />
      Drop a folder anywhere on the window to add it
    </div>

    <footer>
      <label class="skip">
        <input type="checkbox" bind:checked={skip} data-testid="skip-onboarding" />
        Don't show this next time
      </label>
      <button class="go" onclick={done} data-testid="onboarding-go">Get started</button>
    </footer>
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
  .lead {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    line-height: 1.6;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
  }
  .steps li {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    line-height: 1.5;
  }
  .n {
    width: 22px;
    height: 22px;
    flex: none;
    display: grid;
    place-items: center;
    border: 1px solid var(--ls-gold);
    border-radius: 50%;
    color: var(--ls-gold-bright);
    font-family: var(--ff-mono);
    font-size: var(--fs-xs);
  }
  .q {
    color: var(--ls-status-unidentified);
    letter-spacing: 0.08em;
  }
  .drop {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    padding: var(--sp-4);
    border: 2px dashed var(--ls-gold);
    border-radius: var(--r-md);
    color: var(--ls-gold-bright);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  footer {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .skip {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
    color: var(--ls-text-muted);
    cursor: pointer;
  }
  .skip input {
    accent-color: var(--ls-gold);
  }
  .go {
    margin-left: auto;
    padding: 8px var(--sp-5);
    background: color-mix(in srgb, var(--ls-gold) 18%, transparent);
    border: 1px solid var(--ls-gold);
    border-radius: var(--r-md);
    color: var(--ls-gold-bright);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: 600;
    cursor: pointer;
  }
  .go:hover {
    background: color-mix(in srgb, var(--ls-gold) 28%, transparent);
  }
</style>
