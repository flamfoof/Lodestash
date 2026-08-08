<script lang="ts">
  import Icon from "./Icon.svelte";
  import Viewer3D from "./Viewer3D.svelte";
  import {
    assetDetail,
    fileUrl,
    previewUrl,
    typeToken,
    typeIcon,
    fmtSize,
    statusLabel,
    type AssetDetail,
  } from "./api";

  let {
    assetId,
    onclose,
    onstep,
  }: {
    assetId: number;
    onclose: () => void;
    /** -1 / +1 — walk the current page without leaving the viewer. */
    onstep: (delta: number) => void;
  } = $props();

  let detail = $state<AssetDetail | null>(null);
  let loadError = $state<string | null>(null);

  $effect(() => {
    const id = assetId;
    let stale = false;
    detail = null;
    loadError = null;
    assetDetail(id).then((d) => {
      if (!stale) detail = d;
    });
    return () => {
      stale = true;
    };
  });

  // Which viewer the media type gets. Anything without a real viewer yet falls back to the
  // stored vector preview or the type mark, rather than an empty pane that looks broken.
  let kind = $derived.by(() => {
    if (!detail) return "loading";
    // A material's shader ball is *generated* — no bytes are read from the pack — so it is the
    // one type that previews fine while still in an archive.
    if (detail.media_type === "material") return "model";
    // In-pack assets preview too: lodestash://file materializes the one payload into the
    // size-capped scratch on demand (FEATURES §5.4). The `not_local` branch below remains for
    // genuine pull failures (over the cap, or the pack vanished).
    switch (detail.media_type) {
      case "image":
      case "texture":
      case "sprite":
        return "image";
      case "audio":
        return "audio";
      case "model_3d":
        return "model";
      default:
        return "other";
    }
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
    else if (e.key === "ArrowRight") onstep(1);
    else if (e.key === "ArrowLeft") onstep(-1);
    else return;
    e.preventDefault();
  }
</script>

<svelte:window on:keydown={onkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={onclose} data-testid="quicklook">
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="sheet"
    style="--tint: var({detail ? typeToken(detail.media_type) : '--ls-text-dim'})"
    onclick={(e) => e.stopPropagation()}
    role="dialog"
    aria-modal="true"
    aria-label="Quick Look"
    tabindex="-1"
  >
    <header>
      <span class="name" data-testid="ql-name">{detail?.name ?? "…"}</span>
      {#if detail}
        <span class="badge">{detail.media_type}</span>
        <span class="badge">{statusLabel(detail.status)}</span>
        <span class="badge num">{fmtSize(detail.size_bytes)}</span>
      {/if}
      <button class="close" onclick={onclose} aria-label="Close">✕</button>
    </header>

    <div class="stage" data-testid="ql-stage" data-kind={kind}>
      {#if kind === "loading"}
        <p class="msg">Loading…</p>
      {:else if kind === "not_local"}
        <div class="msg stack">
          <Icon name="ls-unitypackage" size={32} />
          <p>Still inside its <code>.unitypackage</code>.</p>
          <p class="dim">Extracting is an explicit action — it never happens just by looking.</p>
        </div>
      {:else if kind === "image"}
        <!-- Straight from lodestash://file: no base64 round-trip through IPC, and the webview
             streams it with Range like any other image. -->
        <img
          src={fileUrl(assetId)}
          alt={detail?.name}
          onerror={() => (loadError = "This file could not be decoded.")}
        />
      {:else if kind === "model" && detail}
      <Viewer3D assetId={detail.asset_id} textureAssetId={detail.texture_asset_id} />
    {:else if kind === "audio"}
        <div class="audio">
          <Icon name="ls-type-sound" size={40} />
          <!-- svelte-ignore a11y_media_has_caption -->
          <audio src={fileUrl(assetId)} controls preload="metadata"></audio>
          <p class="dim">Scrubbing issues Range requests against <code>lodestash://file</code>.</p>
        </div>
      {:else if detail?.has_preview}
        <img src={previewUrl(assetId)} alt="" class="vector" />
      {:else}
        <div class="msg stack">
          <Icon name={typeIcon(detail?.media_type ?? "")} size={40} />
          <p class="dim">No viewer for this type yet.</p>
        </div>
      {/if}
      {#if loadError}
        <p class="msg danger overlay">{loadError}</p>
      {/if}
    </div>

    <footer>
      <button onclick={() => onstep(-1)} aria-label="Previous">‹</button>
      <span class="path" title={detail?.abs_path}>{detail?.rel_path ?? ""}</span>
      <button onclick={() => onstep(1)} aria-label="Next">›</button>
    </footer>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.6);
    backdrop-filter: blur(2px);
  }
  .sheet {
    display: flex;
    flex-direction: column;
    width: min(900px, 88vw);
    height: min(640px, 84vh);
    background: var(--ls-surface-raised);
    border: 1px solid var(--ls-border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--el-popover-shadow);
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-3) var(--sp-4);
    border-bottom: 1px solid var(--ls-border);
  }
  .name {
    font-weight: 600;
    color: var(--ls-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    font-size: var(--fs-xs);
    color: var(--ls-text-muted);
    border: 1px solid var(--ls-border);
    border-radius: var(--r-pill);
    padding: 1px var(--sp-2);
    flex: none;
  }
  .close {
    margin-left: auto;
    color: var(--ls-text-dim);
    font-size: var(--fs-lg);
    line-height: 1;
  }
  .close:hover {
    color: var(--ls-text);
  }

  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    /* Sunken recess behind previews, per the prototype. */
    background: var(--ls-bg);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--tint) 18%, transparent);
    padding: var(--sp-4);
  }
  .stage :global(.wrap) {
    justify-self: stretch;
    align-self: stretch;
  }
  .stage img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .stage img.vector {
    width: 60%;
  }
  .audio {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-3);
  }
  .audio audio {
    width: min(520px, 70vw);
  }

  .msg {
    color: var(--ls-text-muted);
    text-align: center;
    margin: 0;
  }
  .msg.stack {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-2);
  }
  .msg.danger {
    color: var(--ls-danger);
  }
  .overlay {
    position: absolute;
    bottom: var(--sp-3);
  }
  .dim {
    color: var(--ls-text-dim);
    font-size: var(--fs-sm);
    margin: 0;
  }
  code {
    font-family: var(--ff-mono);
    font-size: var(--fs-xs);
  }

  footer {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-4);
    border-top: 1px solid var(--ls-border);
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
  }
  footer button {
    color: var(--ls-text-muted);
    font-size: var(--fs-lg);
    line-height: 1;
    padding: 0 var(--sp-2);
  }
  footer button:hover {
    color: var(--ls-gold);
  }
  .path {
    flex: 1;
    text-align: center;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }
</style>
