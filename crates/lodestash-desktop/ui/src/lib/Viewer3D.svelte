<script lang="ts">
  import { Viewer3D } from "./viewer3d";
  import { meshUrl, fileUrl, slotTexUrl } from "./api";

  let {
    assetId,
    textureAssetId = null,
  }: { assetId: number; textureAssetId?: number | null } = $props();

  let canvas = $state<HTMLCanvasElement | null>(null);
  let status = $state<"loading" | "ready" | "error">("loading");
  let message = $state("");
  let viewer: Viewer3D | null = null;
  let dragging = $state(false);
  /// How many material slots have their texture on the GPU — the e2e suite reads it, and it is the
  /// one observable difference between a painted model and a flat one.
  let painted = $state(0);
  let slotCount = $state(-1);
  let texErr = $state("");

  $effect(() => {
    const el = canvas;
    const id = assetId;
    const texId = textureAssetId;
    if (!el) return;

    let disposed = false;
    painted = 0;
    slotCount = -1;
    texErr = "";
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
      if (disposed) return;
      const paint = (src: string, slot?: number) => {
        const img = new Image();
        img.crossOrigin = "anonymous";
        img.onload = () => {
          if (disposed) return;
          try {
            v.setTexture(img, slot);
            painted += 1;
          } catch (e) {
            texErr = e instanceof Error ? e.message : String(e);
          }
        };
        img.onerror = () => {
          if (!disposed && !texErr) texErr = `load failed: ${src}`;
        };
        // A 204 (this slot is unpainted) decodes as nothing and simply never lands.
        img.src = src;
      };
      // A model split by material slot asks per slot (v1 plan S08); anything else — a material's
      // shader ball — wears the single albedo the Inspector resolved.
      const slots = v.slots();
      slotCount = slots.length;
      if (slots.length > 0) {
        for (const slot of slots) paint(slotTexUrl(id, slot), slot);
      } else if (texId != null) {
        paint(fileUrl(texId));
      }
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

<div class="wrap" data-testid="viewer3d" data-status={status} data-textures={painted} data-slots={slotCount} data-texerr={texErr}>
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
