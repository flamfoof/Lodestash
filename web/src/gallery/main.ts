import "../styles/app.css";
import { mount } from "svelte";
import Gallery from "./Gallery.svelte";
import ShellSpecimen from "./ShellSpecimen.svelte";
import { keymap } from "../app/keymap.svelte";
import { setAppRoot } from "../app/overlay";

const target = document.getElementById("gallery");
if (!target) throw new Error("#gallery missing from gallery.html");

keymap.attach(window);
setAppRoot(target);

// gallery.html#shell is the shell alone, full-window (the gate measures the frame there).
export default location.hash === "#shell" ? mount(ShellSpecimen, { target }) : mount(Gallery, { target });
