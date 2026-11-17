import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import ShellSpecimen from "../gallery/ShellSpecimen.svelte";

let app: ReturnType<typeof mount> | null = null;

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
});

function render() {
  app = mount(ShellSpecimen, { target: document.body });
  flushSync();
}

const navRows = () => [...document.querySelectorAll<HTMLButtonElement>('nav[aria-label="Main"] button')];
const label = (row: Element) => row.querySelector(".label")?.textContent?.trim();

describe("Shell", () => {
  it("lists the eleven destinations in FEATURES §0.3 order", () => {
    render();
    expect(navRows().map(label)).toEqual([
      "Library",
      "Favorites",
      "Identify",
      "Collections",
      "Packages",
      "Unity Assets",
      "Traits",
      "Stats",
      "Duplicates",
      "Trash",
      "Settings",
    ]);
  });

  it("marks the current destination, and moves it on navigation", () => {
    render();
    const current = () => navRows().filter((r) => r.getAttribute("aria-current") === "page").map(label);
    expect(current()).toEqual(["Library"]);
    navRows()[3].click();
    flushSync();
    expect(current()).toEqual(["Collections"]);
  });

  it("badges only Identify, Duplicates and Trash, and only when non-zero", () => {
    render();
    const badged = navRows().filter((r) => r.querySelector(".badge")).map((r) => [label(r), r.querySelector(".badge")?.textContent]);
    // The fixture has 5 unidentified, 1 duplicate group and an empty Trash; Favorites never has one.
    expect(badged).toEqual([
      ["Identify", "5"],
      ["Duplicates", "1"],
    ]);
  });

  it("shows the facet groups with their counts, the chosen one pressed", () => {
    render();
    const pressed = [...document.querySelectorAll('aside [aria-pressed="true"]')].map(label);
    expect(pressed).toEqual(["All", "All libraries"]);
    const status = document.querySelector('[aria-labelledby$="-status"]')!;
    expect([...status.querySelectorAll(".count")].map((c) => c.textContent)).toEqual(["19", "5", "9", "5"]);
  });

  it("puts the category tabs in FEATURES §0.2 order, Materials included", () => {
    render();
    const tabs = [...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent?.trim());
    expect(tabs).toEqual(["All", "Models", "Materials", "Sounds", "Images"]);
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent?.trim()).toBe("All");
  });
});
