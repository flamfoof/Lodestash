import { defineConfig, type Plugin } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const repo = resolve(__dirname, "..");
const SPRITE = resolve(repo, "docs/design/icons/icons.svg");

/**
 * One icon sprite, single-sourced from docs/design/icons/icons.svg (ADR-0012). The designer's
 * export bakes a hex stroke into every symbol; the app tints icons with `currentColor`, so the
 * served/emitted copy rewrites colour attributes. The source file is never modified.
 */
export function iconSprite(): Plugin {
  const tinted = () =>
    readFileSync(SPRITE, "utf8").replace(/\b(stroke|fill)="#[0-9a-fA-F]{3,8}"/g, '$1="currentColor"');
  return {
    name: "lodestash-icon-sprite",
    configureServer(server) {
      server.watcher.add(SPRITE);
      server.middlewares.use("/icons.svg", (_req, res) => {
        res.setHeader("content-type", "image/svg+xml");
        res.end(tinted());
      });
    },
    generateBundle() {
      this.emitFile({ type: "asset", fileName: "icons.svg", source: tinted() });
    },
  };
}

// The UI talks to the app's engine over /v1. In dev, Vite proxies to the engine the app started
// (LODESTASH_ENGINE=http://127.0.0.1:<port>, printed by `lodestash-desktop --serve`).
const engine = process.env.LODESTASH_ENGINE;

export default defineConfig({
  plugins: [svelte(), iconSprite()],
  clearScreen: false,
  server: {
    port: 5180,
    strictPort: true,
    // tokens.css is imported from docs/design — the one checked-in source.
    fs: { allow: [repo] },
    headers: {
      // Offline invariant: no remote font, script or style, in dev as in the app.
      "Content-Security-Policy":
        "default-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self' ws:",
    },
    proxy: engine
      ? {
          "/v1": { target: engine, changeOrigin: true },
          "/session": { target: engine, changeOrigin: true },
        }
      : undefined,
  },
  build: {
    target: "chrome120",
    sourcemap: true,
    rollupOptions: {
      input: { main: resolve(__dirname, "index.html"), gallery: resolve(__dirname, "gallery.html") },
    },
  },
});
