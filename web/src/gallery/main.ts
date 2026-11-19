import "../styles/app.css";
import { mount, type ComponentProps } from "svelte";
import Gallery from "./Gallery.svelte";
import LibrarySpecimen from "./LibrarySpecimen.svelte";
import ShellSpecimen from "./ShellSpecimen.svelte";
import { keymap } from "../app/keymap.svelte";
import { setAppRoot } from "../app/overlay";

const target = document.getElementById("gallery");
if (!target) throw new Error("#gallery missing from gallery.html");

keymap.attach(window);
setAppRoot(target);

type LibraryProps = ComponentProps<typeof LibrarySpecimen>;

/**
 * The Library specimen pages, by hash: each starts the screen in one state of the design contract
 * (docs/design/contract.json), which the gate measures full-window with positions.
 */
const LIBRARY: Record<string, LibraryProps> = {
  // contract state `library`
  "#library": {},
  // `library+inspector`: the first asset selected
  "#library-inspector": { select: 1 },
  // `library+filters`: ⚑ open with one value chosen in each group
  "#library-filters": { filters: { traits: ["rock"], tris: ["low"], sizes: ["M"] }, open: "filters" },
  // `library+picks`: the first three cards picked — the bulk bar
  "#library-picks": { picks: [1, 2, 3] },
  // `library+list`: the view toggle's second stop
  "#library-list": { layout: "list" },
};

// `#shell` is the frame alone; anything else that is not a Library page is the primitive sheet.
function page() {
  const library = LIBRARY[location.hash];
  if (library) return mount(LibrarySpecimen, { target: target!, props: library });
  if (location.hash === "#shell") return mount(ShellSpecimen, { target: target! });
  return mount(Gallery, { target: target! });
}

export default page();
