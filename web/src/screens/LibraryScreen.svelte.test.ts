// Runes are allowed here (the file name carries .svelte): a test needs reactive props to change them.
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { keymap } from "../app/keymap.svelte";
import type { AssetSummary } from "../features/asset/model";
import { DEFAULT_VIEW } from "../features/library/view";
import { ASSETS, PROJECT, SIZE_THRESHOLDS } from "../fixtures/library";
import { Selection } from "../state/selection.svelte";
import LibraryScreen from "./LibraryScreen.svelte";

let app: ReturnType<typeof mount> | null = null;

beforeEach(() => keymap.reset());

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
});

describe("LibraryScreen — the selection follows the result", () => {
  it("forgets a lead and picks that are no longer on the page", () => {
    const selection = new Selection();
    selection.moveTo(6);
    selection.toggle(2);
    selection.toggle(7);
    const props = $state({ assets: ASSETS as AssetSummary[], view: DEFAULT_VIEW, selection, project: PROJECT, thresholds: SIZE_THRESHOLDS });
    app = mount(LibraryScreen, { target: document.body, props });
    flushSync();
    expect(selection.lead).toBe(6);
    props.assets = ASSETS.filter((a) => a.id <= 5);
    flushSync();
    expect(document.querySelectorAll('[role="gridcell"]').length).toBe(5);
    expect([selection.lead, [...selection.picks]]).toEqual([null, [2]]);
  });

  it("re-renders a card from the asset it is handed — the screen keeps no copy", () => {
    const selection = new Selection();
    const props = $state({ assets: ASSETS as AssetSummary[], view: DEFAULT_VIEW, selection, project: PROJECT, thresholds: SIZE_THRESHOLDS });
    app = mount(LibraryScreen, { target: document.body, props });
    flushSync();
    const star = () => document.querySelector('[role="gridcell"][data-id="2"] .tile[aria-pressed]')?.getAttribute("aria-pressed");
    expect(star()).toBe("false");
    props.assets = ASSETS.map((a) => (a.id === 2 ? { ...a, favorite: true } : a));
    flushSync();
    expect(star()).toBe("true");
  });
});
