<script lang="ts">
  import { isControl, type KeyHandler } from "../../app/keymap.svelte";
  import type { Playback } from "../../state/playback";
  import { AudioTransport, Button, Dialog, IconButton, PanelTitle, StatList, StatusLine, Text, TextLink, Thumb, TraitChip, TypeMark, Waveform } from "../../ui";
  import { intentFor, whyNot, type AssetActionId, type AssetIntent } from "../asset/actions";
  import type { AssetSummary, ProjectRef, TraitDef } from "../asset/model";
  import { chipOf, formatClock, importedState, peekLabel, statRows, typeTone, userStatus } from "../asset/present";

  /**
   * Quick Look (UI-SPEC §14, FEATURES §7.3): a closer look at one asset without leaving where you
   * were. A `Dialog`, so the scrim, the focus trap, the inert page behind it and the return of
   * focus are the dialog's; what is here is what it shows and which keys it adds.
   *
   * **Left**, what is being looked at, by type: a picture whole (never cropped, always at full
   * detail); a model's pre-render with its caption — the surface `features/viewer3d`'s orbit
   * viewport will fill; a sound's waveform over the transport. **Right**, what it is: name, status,
   * the stat block, its traits (read-only here), and **Stage / Import** for an identified asset,
   * through `actions.ts` like every other place that offers them.
   *
   * **Keys**, bound in the dialog's scope so nothing global answers behind it: `Esc` closes (the
   * dialog's own), `Space` closes — Space is the toggle that opened it — and `←` `→` step. Each is
   * left to a control that uses it: Space to a focused button, the arrows to the scrubber.
   *
   * It holds nothing: `asset` is what to show (null: closed), `playback` and `peaks` come from
   * whoever owns playback, and stepping, seeking and closing are reported. The owner decides what
   * "next" is.
   */
  let {
    asset,
    project,
    traits = [],
    playback = null,
    peaks = null,
    onintent,
    onstep,
    onseek,
    onclose,
  }: {
    /** The asset shown; null when Quick Look is closed. */
    asset: AssetSummary | null;
    project: ProjectRef | null;
    traits?: readonly TraitDef[];
    /** What is playing, if anything. Only its state for this asset is shown. */
    playback?: Playback | null;
    /** This sound's waveform, as shares of the band's height; null while not known. */
    peaks?: readonly number[] | null;
    onintent?: (intent: AssetIntent) => void;
    /** Show the previous (-1) or the next (1) asset of the visible set. */
    onstep?: (dir: 1 | -1) => void;
    /** Move playback of this sound to `seconds`. */
    onseek?: (seconds: number) => void;
    onclose?: () => void;
  } = $props();

  const status = $derived(asset ? userStatus(asset.status) : "unidentified");
  const unidentified = $derived(status === "unidentified");
  const sound = $derived(asset?.media_type === "audio");
  const ctx = $derived({ targets: asset ? [asset] : [], project });
  const imported = $derived(asset ? importedState(asset, project) : null);
  /** This asset's playback: someone else's sound playing is not this one's state. */
  const mine = $derived(asset && playback?.id === asset.id ? playback : null);
  const duration = $derived(asset?.stats.duration_s ?? 0);

  function run(id: AssetActionId) {
    const intent = intentFor(id, ctx);
    if (intent) onintent?.(intent);
  }

  const scrubbing = (event: KeyboardEvent) => event.target instanceof HTMLInputElement && event.target.type === "range";
  const step =
    (dir: 1 | -1): KeyHandler =>
    (event) =>
      scrubbing(event) ? false : onstep?.(dir);
  const keys: Record<string, KeyHandler> = {
    Space: (event) => (isControl(event.target) ? false : onclose?.()),
    ArrowLeft: step(-1),
    ArrowRight: step(1),
  };
</script>

<Dialog
  open={asset !== null}
  title={asset ? `Quick Look: ${asset.name}` : "Quick Look"}
  size="wide"
  bare
  {keys}
  onclose={() => onclose?.()}
  scrimAttrs={{ "data-anchor": "quicklook.scrim" }}
  panelAttrs={{ "data-anchor": "quicklook" }}
>
  {#if asset}
    <Thumb size="quicklook" src={sound ? null : asset.preview_url} unknown={unidentified} data-anchor="quicklook.preview" data-media={asset.media_type}>
      {#if sound}
        <div class="sound">
          <Waveform
            peaks={peaks ?? []}
            played={mine && duration > 0 ? mine.position / duration : undefined}
            data-anchor="quicklook.wave"
            barAttrs={{ first: { "data-anchor": "quicklook.wave.bar" }, last: { "data-anchor": "quicklook.wave.bar.last" } }}
          />
          <AudioTransport
            name={asset.name}
            playing={mine?.playing ?? false}
            position={mine?.position ?? 0}
            {duration}
            time={formatClock}
            anchors={{ play: "quicklook.play", scrub: "quicklook.scrub", time: "quicklook.time" }}
            ontoggle={() => run("play")}
            onseek={(seconds) => onseek?.(seconds)}
            data-anchor="quicklook.transport"
          />
        </div>
      {:else if asset.media_type === "model_3d" && !unidentified}
        <span class="caption"><Text variant="caption" data-anchor="quicklook.caption">low-res pre-render · orbit viewport in app</Text></span>
      {/if}
    </Thumb>

    <div class="body" data-anchor="quicklook.body">
      <header>
        <div class="name">
          <TypeMark tone={typeTone(asset.media_type)} size="md" data-anchor="quicklook.type" />
          <PanelTitle text={asset.name} size="lg" unknown={unidentified} data-anchor="quicklook.name" />
          <span class="close">
            <IconButton icon="action-clear" label="Close Quick Look" onclick={() => onclose?.()} data-anchor="quicklook.close" />
          </span>
        </div>
        <StatusLine
          {status}
          suffix={peekLabel(asset.media_type)}
          rule
          data-anchor="quicklook.status"
          suffixAttrs={{ "data-anchor": "quicklook.status.type" }}
          ruleAttrs={{ "data-anchor": "quicklook.rule" }}
        />
      </header>

      <StatList rows={statRows(asset)} anchors={{ row: "quicklook.stat", key: "quicklook.stat.key", value: "quicklook.stat.value" }} />

      {#if !unidentified && asset.tags.length > 0}
        <div class="chips" role="group" aria-label="Traits">
          {#each asset.tags as tag, i (tag.source + tag.value)}
            {@const chip = chipOf(tag, traits)}
            <TraitChip
              name={chip.name}
              source={chip.source}
              confidence={chip.confidence}
              color={chip.color}
              icon={chip.icon}
              size="md"
              data-anchor={i === 0 ? "quicklook.chip.lead" : i === 1 ? "quicklook.chip" : undefined}
            />
          {/each}
        </div>
      {/if}

      <div class="foot">
        {#if !unidentified}
          <div class="pair">
            <Button
              variant="outline-gold"
              size="block"
              icon={asset.staged ? "status-identified" : "action-stage"}
              iconSize="13"
              iconBox="11"
              aria-pressed={asset.staged}
              onclick={() => run("stage")}
              data-anchor="quicklook.stage"
            >
              {asset.staged ? "Staged" : "Stage"}
            </Button>
            <Button
              variant="primary"
              size="block"
              icon={imported?.tone === "active" ? "status-identified" : "import"}
              iconSize="13"
              iconBox="11"
              disabled={whyNot("import", ctx) !== undefined}
              title={whyNot("import", ctx) ?? (imported?.tone === "active" ? "Already in this project — importing again asks first" : undefined)}
              onclick={() => run("import")}
              data-anchor="quicklook.import"
            >
              {imported?.tone === "active" ? "Imported" : "Import"}
            </Button>
          </div>
        {/if}
        <div class="hints">
          <TextLink tone="dim" hover="text" size="10" title="Previous" onclick={() => onstep?.(-1)} data-anchor="quicklook.prev">← prev</TextLink>
          <Text variant="note" data-anchor="quicklook.hint">Esc closes · ← → step</Text>
          <TextLink tone="dim" hover="text" size="10" title="Next" onclick={() => onstep?.(1)} data-anchor="quicklook.next">next →</TextLink>
        </div>
      </div>
    </div>
  {/if}
</Dialog>

<style>
  /* Layout only: the two panes' insides. */
  .sound {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-22);
    width: 100%;
    height: 100%;
    padding: 0 var(--space-26);
  }

  .caption {
    position: absolute;
    left: var(--space-12);
    bottom: var(--space-10);
    display: flex;
  }

  .body {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-12);
    min-width: 0;
    padding: var(--space-18);
  }

  .name {
    display: flex;
    align-items: center;
    gap: var(--space-8);
  }

  .close {
    display: flex;
    flex: none;
    margin-left: auto;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-5);
  }

  .foot {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    margin-top: auto;
  }

  .pair {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-8);
  }

  .hints {
    display: flex;
    justify-content: space-between;
  }
</style>
