import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Unit tests for primitives and app plumbing (plan P6 Task 6.3): a DOM without layout (jsdom), so
// geometry is stubbed where a test needs it; the contract gate (e2e/gate.mjs) owns real layout.
export default defineConfig({
  plugins: [svelte()],
  resolve: { conditions: ["browser"] },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
    // A hang is a failure (owner's convention): no test may wait longer than this.
    testTimeout: 5000,
  },
});
