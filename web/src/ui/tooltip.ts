// `{@attach tooltip("Show only favorites")}` — a hover/focus label for any element.
//
// Shown after a short delay on pointer hover and at once on keyboard focus; hidden on leave, blur
// or Escape. While it is up the element is `aria-describedby` the bubble, so a screen reader reads
// it too. One bubble at a time.

import { mount, unmount } from "svelte";
import { overlayHost } from "../app/overlay";
import { boxOf, place, viewportBox } from "./position";
import Tooltip from "./Tooltip.svelte";

const HOVER_DELAY_MS = 400; // --dur-400: the prototype's reveal duration
let seq = 0;
let current: (() => void) | null = null;

export function tooltip(text: string) {
  return (node: HTMLElement) => {
    const id = `ls-tip-${++seq}`;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let bubble: ReturnType<typeof mount> | null = null;

    const show = () => {
      clearTimeout(timer);
      if (bubble) return;
      current?.();
      // Mounted hidden, measured, then placed: its size is only known once it has rendered.
      bubble = mount(Tooltip, { target: overlayHost(), props: { id, text } });
      const el = document.getElementById(id);
      if (el) {
        const r = el.getBoundingClientRect();
        const at = place(boxOf(node), { width: r.width, height: r.height }, viewportBox(), { align: "center", gap: 6 });
        el.style.left = `${at.x}px`;
        el.style.top = `${at.y}px`;
        el.style.visibility = "visible";
      }
      const described = node.getAttribute("aria-describedby");
      node.setAttribute("aria-describedby", described ? `${described} ${id}` : id);
      current = hide;
    };
    const hide = () => {
      clearTimeout(timer);
      if (!bubble) return;
      unmount(bubble);
      bubble = null;
      const rest = (node.getAttribute("aria-describedby") ?? "").split(" ").filter((t) => t && t !== id);
      if (rest.length) node.setAttribute("aria-describedby", rest.join(" "));
      else node.removeAttribute("aria-describedby");
      if (current === hide) current = null;
    };
    const hoverIn = () => {
      clearTimeout(timer);
      timer = setTimeout(show, HOVER_DELAY_MS);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && bubble) hide();
    };

    node.addEventListener("pointerenter", hoverIn);
    node.addEventListener("pointerleave", hide);
    node.addEventListener("focusin", show);
    node.addEventListener("focusout", hide);
    node.addEventListener("keydown", onKey);
    return () => {
      hide();
      node.removeEventListener("pointerenter", hoverIn);
      node.removeEventListener("pointerleave", hide);
      node.removeEventListener("focusin", show);
      node.removeEventListener("focusout", hide);
      node.removeEventListener("keydown", onKey);
    };
  };
}
