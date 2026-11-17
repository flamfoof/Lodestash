// node --test scripts/design/check-styles.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { checkFile } from "./check-styles.mjs";

const svelte = (css) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ls-styles-"));
  const file = path.join(dir, "X.svelte");
  fs.writeFileSync(file, `<div></div>\n<style>\n  .x {\n${css}\n  }\n</style>\n`);
  return file;
};
const props = (file) => checkFile(file).map((f) => f.prop);

test("tokens, 0, 1px, percentages and fr pass", () => {
  const f = svelte(
    [
      "    color: var(--ls-text);",
      "    padding: 0 var(--space-10);",
      "    border: 1px solid var(--ls-border);",
      "    width: 100%;",
      "    grid-template-columns: var(--sidebar-w) 1fr;",
      "    box-shadow: var(--glow-gold-14-a50);",
      "    transition: border-color var(--dur-120) ease;",
      "    z-index: var(--z-dialog);",
      "    opacity: 0;",
      "    font-weight: var(--fw-600);",
    ].join("\n"),
  );
  assert.deepEqual(checkFile(f), []);
});

test("each kind of literal is reported with its line", () => {
  const f = svelte(
    [
      "    color: #e7b765;",
      "    background: rgba(0, 0, 0, 0.5);",
      "    padding: 8px 12px;",
      "    z-index: 50;",
      "    transition: opacity 120ms ease;",
      "    box-shadow: 0 0 8px black;",
      "    opacity: 0.5;",
      "    font-weight: 600;",
      "    margin: var(--space-10, 10px);",
      "    letter-spacing: 0.5px;",
    ].join("\n"),
  );
  assert.deepEqual(props(f), [
    "color", "background", "padding", "z-index", "transition", "box-shadow", "opacity", "font-weight", "margin",
    "letter-spacing",
  ]);
  assert.equal(checkFile(f)[0].line, 4);
});

test("the escape hatch needs a reason", () => {
  const ok = svelte("    color: #fff; /* style-lint-allow: printing on white */");
  const bare = svelte("    color: #fff; /* style-lint-allow: */");
  assert.deepEqual(checkFile(ok), []);
  assert.equal(checkFile(bare).length, 1);
});

test("css outside <style> is not read", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ls-styles-"));
  const file = path.join(dir, "Y.svelte");
  fs.writeFileSync(file, `<script>\n  const c = "color: #fff;";\n</script>\n<p>padding: 8px;</p>\n`);
  assert.deepEqual(checkFile(file), []);
});
