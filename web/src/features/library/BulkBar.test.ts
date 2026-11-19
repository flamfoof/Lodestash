import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ASSETS, PROJECT } from "../../fixtures/library";
import { intentFor, whyNot, type AssetIntent } from "../asset/actions";
import type { AssetSummary } from "../asset/model";
import BulkBar from "./BulkBar.svelte";

let app: ReturnType<typeof mount> | null = null;

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
});

const byId = (...ids: number[]) => ASSETS.filter((a) => ids.includes(a.id)) as AssetSummary[];

function render(targets: AssetSummary[]) {
  const intents: AssetIntent[] = [];
  const onclear = vi.fn();
  app = mount(BulkBar, { target: document.body, props: { targets, project: PROJECT, onintent: (i: AssetIntent) => intents.push(i), onclear } });
  flushSync();
  return { intents, onclear };
}

const bar = () => document.querySelector<HTMLElement>('[role="toolbar"][aria-label="Bulk actions"]');
const button = (text: string) => [...bar()!.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.includes(text))!;
const field = () => bar()!.querySelector<HTMLInputElement>("input")!;

function type(text: string) {
  field().value = text;
  field().dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}

function click(text: string) {
  button(text).click();
  flushSync();
}

describe("BulkBar", () => {
  it("is not rendered with nothing picked, and counts what is", () => {
    render([]);
    expect(bar()).toBeNull();
    if (app) unmount(app);
    render(byId(1, 2, 3));
    expect(bar()!.textContent).toContain("3 selected");
  });

  it("Add tag and Remove tag emit the intents actions.ts produces for the pick set", () => {
    const targets = byId(1, 2, 3);
    const { intents } = render(targets);
    type("  hero  ");
    click("Add tag");
    click("Remove tag");
    const ctx = { targets, project: PROJECT };
    expect(intents).toEqual([intentFor("tag-add", ctx, { tag: "hero" }), intentFor("tag-remove", ctx, { tag: "hero" })]);
    expect([intents[0].action, intents[0].ids, intents[0].tag]).toEqual(["tag-add", [1, 2, 3], "hero"]);
  });

  it("a tag edit leaves Unidentified assets out of the intent", () => {
    // 5 and 6 are unidentified.
    const { intents } = render(byId(1, 5, 6, 7));
    type("prop");
    click("Add tag");
    expect(intents[0].ids).toEqual([1, 7]);
    expect(button("Add tag").title).toContain("2 Unidentified are skipped");
  });

  it("Enter in the field adds", () => {
    const { intents } = render(byId(2, 3));
    type("stone");
    field().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(intents.map((i) => [i.action, i.tag])).toEqual([["tag-add", "stone"]]);
  });

  it("an empty name emits nothing, says why, and puts focus in the field", () => {
    const { intents } = render(byId(1, 2));
    click("Add tag");
    expect(intents).toEqual([]);
    expect(bar()!.querySelector('[role="status"]')?.textContent).toBe(whyNot("tag-add", { targets: byId(1, 2), project: PROJECT }, { tag: "" }));
    expect(document.activeElement).toBe(field());
    type("x");
    expect(bar()!.querySelector('[role="status"]')).toBeNull();
  });

  it("a set of only Unidentified assets cannot be tagged, and says so", () => {
    const { intents } = render(byId(5, 6));
    type("prop");
    click("Remove tag");
    expect(intents).toEqual([]);
    expect(bar()!.querySelector('[role="status"]')?.textContent).toMatch(/Unidentified assets are skipped/);
  });

  it("Stage all is the one stage toggle: it stages, and on a staged set unstages", () => {
    const targets = byId(1, 2, 8);
    const { intents } = render(targets);
    click("Stage all");
    expect(intents).toEqual([intentFor("stage", { targets, project: PROJECT })]);
    expect([intents[0].ids, intents[0].on]).toEqual([[1, 2, 8], true]);
    if (app) unmount(app);
    const staged = targets.map((a) => ({ ...a, staged: true }));
    const again = render(staged);
    click("Unstage all");
    expect(again.intents).toEqual([intentFor("stage", { targets: staged, project: PROJECT })]);
    expect([again.intents[0].ids, again.intents[0].on]).toEqual([[1, 2, 8], false]);
  });

  it("Clear reports and emits no intent", () => {
    const { intents, onclear } = render(byId(1, 2));
    click("Clear");
    expect(onclear).toHaveBeenCalledOnce();
    expect(intents).toEqual([]);
  });
});
