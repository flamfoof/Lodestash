// node --test scripts/design/check-tokens.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { check, canonColor, canonShadow, canonGradient } from "./check-tokens.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const css = fs.readFileSync(path.join(ROOT, "docs", "design", "tokens.css"), "utf8");
const vocab = JSON.parse(fs.readFileSync(path.join(ROOT, "docs", "design", "vocabulary.json"), "utf8"));

test("the committed tokens name every drawn value and nothing else", () => {
  assert.deepEqual(check(css, vocab).problems, []);
});

test("removing a drawn colour's token is reported with the value", () => {
  const without = css.replace(/^\s*--ls-thumb:.*$/m, "");
  const { problems } = check(without, vocab);
  assert.ok(problems.some((p) => p.includes("drawn, no token: color #20293a")), problems.join("\n"));
});

test("a token nothing draws is reported unless it is in an app-only block", () => {
  const extra = css.replace(":root {", ":root {\n  --ls-made-up: #123456;");
  assert.ok(check(extra, vocab).problems.some((p) => p.includes("--ls-made-up")));
  const exempt = css.replace(":root {", ":root {\n  /* app-only: test */\n  --ls-made-up: #123456;\n  /* end app-only */");
  assert.deepEqual(check(exempt, vocab).problems, []);
});

test("a legacy token does not satisfy a drawn value", () => {
  // --ls-thumb removed; a legacy token with the same value must not stand in for it.
  const moved = css.replace(/^\s*--ls-thumb:.*$/m, "").replace("/* end legacy */", "--old-thumb: #20293a;\n  /* end legacy */");
  assert.ok(check(moved, vocab).problems.some((p) => p.includes("#20293a")));
});

test("canonical forms agree between computed and authored CSS", () => {
  assert.equal(canonColor("rgba(231, 183, 101, 0.15)"), "#e7b76526");
  assert.equal(canonColor("#E7B765"), "#e7b765");
  assert.equal(canonColor("rgba(0, 0, 0, 0)"), null);
  assert.equal(canonShadow("rgba(231, 183, 101, 0.4) 0px 0px 0px 1px inset"), canonShadow("inset 0 0 0 1px #e7b76566"));
  assert.equal(
    canonGradient("linear-gradient(rgb(231, 183, 101), rgb(217, 165, 78))"),
    canonGradient("linear-gradient(180deg,#E7B765,#D9A54E)"),
  );
});
