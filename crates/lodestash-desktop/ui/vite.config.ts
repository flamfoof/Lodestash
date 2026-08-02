import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri drives the dev server; fail loudly on a port clash instead of silently
// moving to 5174, which the Rust side would not be pointed at.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "chrome105", sourcemap: true },
});
