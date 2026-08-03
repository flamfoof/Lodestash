<script lang="ts">
  import { Viewer3D } from "./viewer3d";
  import { meshUrl, fileUrl } from "./api";

  let {
    assetId,
    textureAssetId = null,
  }: { assetId: number; textureAssetId?: number | null } = $props();

  let canvas = $state<HTMLCanvasElement | null>(null);
  let status = $state<"loading" | "ready" | "error">("loading");
  let message = $state("");
  let viewer: Viewer3D | null = null;
  let dragging = $state(false);

  $effect(() => {
    const el = canvas;
    const id = assetId;
    const texId = textureAssetId;
    if (!el) return;

    let disposed = false;
    status = "loading";
    message = "";

    try {
      viewer?.dispose();
      viewer = new Viewer3D(el);
    } catch (e) {
      status = "error";
      message = "3D preview unavailable (no WebGL2).";
      return;
    }
    const v = viewer;

    (async () => {
      try {
        const res = await fetch(meshUrl(id));
        if (res.status === 503) throw new Error("Still inside its .unitypackage.");
        if (!res.ok) throw new Error(`Mesh unavailable (${res.status}).`);
        const buf = await res.arrayBuffer();
        if (disposed) return;
        v.load(buf);
        status = "ready";
      } catch (e) {
        if (!disposed) {
          status = "error";
          message = e instanceof Error ? e.message : String(e);
        }
        return;
      }

      // Texture is a second, optional step: geometry shows immediately and the albedo lands when
      // it arrives, rather than holding the whole viewer on a texture that may not exist.
      if (texId == null || disposed) return;
      const img = new Image();
      img.crossOrigin = "anonymous";
      img.onload = () => {
        if (!disposed) v.setTexture(img);
      };
      img.src = fileUrl(texId);
    })();

    return () => {
      disposed = true;
      v.dispose();
      if (viewer === v) viewer = null;
    };
  });

  function onpointerdown(e: PointerEvent) {
    dragging = true;
    (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
  }
  function onpointermove(e: PointerEvent) {
    if (dragging) viewer?.orbit(e.movementX, e.movementY);
  }
  function onpointerup(e: PointerEvent) {
    dragging = false;
    (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
  }
  function onwheel(e: WheelEvent) {
    e.preventDefault();
    viewer?.zoom(-e.deltaY);
  }
</script>

<div class="wrap" data-testid="viewer3d" data-status={status}>
  <canvas
    bind:this={canvas}
    data-testid="viewer3d-canvas"
    {onpointerdown}
    {onpointermove}
    {onpointerup}
    onpointercancel={onpointerup}
    {onwheel}
    class:grabbing={dragging}
  ></canvas>
  {#if status === "loading"}
    <p class="overlay">Loading mesh…</p>
  {:else if status === "error"}
    <p class="overlay danger">{message}</p>
  {:else}
    <p class="hint">drag to orbit · scroll to zoom</p>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    width: 100%;
    height: 100%;
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
    cursor: grab;
    touch-action: none;
  }
  canvas.grabbing {
    cursor: grabbing;
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    margin: 0;
    color: var(--ls-text-muted);
    pointer-events: none;
  }
  .overlay.danger {
    color: var(--ls-danger);
  }
  .hint {
    position: absolute;
    bottom: var(--sp-2);
    left: 0;
    right: 0;
    text-align: center;
    margin: 0;
    font-size: var(--fs-xs);
    color: var(--ls-text-dim);
    pointer-events: none;
  }
</style>
