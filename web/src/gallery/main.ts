import "../styles/app.css";
import { mount } from "svelte";
import Gallery from "./Gallery.svelte";
import { keymap } from "../app/keymap.svelte";
import { setAppRoot } from "../app/overlay";

const target = document.getElementById("gallery");
if (!target) throw new Error("#gallery missing from gallery.html");

keymap.attach(window);
setAppRoot(target);

export default mount(Gallery, { target });
