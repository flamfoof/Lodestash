import "./styles/app.css";
import { mount } from "svelte";
import App from "./App.svelte";
import { keymap } from "./app/keymap.svelte";
import { setAppRoot } from "./app/overlay";

const target = document.getElementById("app");
if (!target) throw new Error("#app missing from index.html");

keymap.attach(window);
setAppRoot(target);

export default mount(App, { target });
