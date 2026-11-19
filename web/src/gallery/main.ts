import "../styles/app.css";
import { mount } from "svelte";
import Gallery from "./Gallery.svelte";
import LibrarySpecimen from "./LibrarySpecimen.svelte";
import ShellSpecimen from "./ShellSpecimen.svelte";
import { keymap } from "../app/keymap.svelte";
import { setAppRoot } from "../app/overlay";

const target = document.getElementById("gallery");
if (!target) throw new Error("#gallery missing from gallery.html");

keymap.attach(window);
setAppRoot(target);

// The specimen pages the gate measures full-window, by hash; anything else is the primitive sheet.
//   #shell               the frame alone
//   #library             the Library screen in the frame (contract state `library`)
//   #library-inspector   the same with the first asset selected (`library+inspector`)
function page() {
  switch (location.hash) {
    case "#shell":
      return mount(ShellSpecimen, { target: target! });
    case "#library":
      return mount(LibrarySpecimen, { target: target! });
    case "#library-inspector":
      return mount(LibrarySpecimen, { target: target!, props: { select: 1 } });
    default:
      return mount(Gallery, { target: target! });
  }
}

export default page();
