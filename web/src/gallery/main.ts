import "../styles/app.css";
import { mount } from "svelte";
import Gallery from "./Gallery.svelte";

const target = document.getElementById("gallery");
if (!target) throw new Error("#gallery missing from gallery.html");

export default mount(Gallery, { target });
