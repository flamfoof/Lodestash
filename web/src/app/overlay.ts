// The overlay host: where dialogs, menus, popovers and tooltips render, outside the app root, so
// no ancestor's overflow or stacking context can clip them (spec §9.1 `app/` overlay host).
//
// Modal layers are a stack. While any is open the app root is `inert`, and so is every modal below
// the top-most one — focus and pointer input reach only the layer the user is looking at.

const HOST_ID = "ls-overlay-host";

let appRoot: HTMLElement | null = null;
const modals: HTMLElement[] = [];

/** Register the element that holds the app (everything an open modal must make inert). */
export function setAppRoot(el: HTMLElement | null): void {
  if (appRoot && appRoot !== el) setInert(appRoot, false);
  appRoot = el;
  syncInert();
}

/** The host element, created on first use as the last child of `<body>`. */
export function overlayHost(): HTMLElement {
  let host = document.getElementById(HOST_ID);
  if (!host) {
    host = document.createElement("div");
    host.id = HOST_ID;
    document.body.appendChild(host);
  }
  return host;
}

/** Svelte action/attachment: move `node` into the overlay host for as long as it is mounted. */
export function portal(node: HTMLElement): { destroy(): void } {
  overlayHost().appendChild(node);
  return { destroy: () => node.remove() };
}

/** Mark `layer` as the top-most modal. Returns the function that removes it again. */
export function pushModal(layer: HTMLElement): () => void {
  modals.push(layer);
  syncInert();
  return () => {
    const at = modals.indexOf(layer);
    if (at >= 0) modals.splice(at, 1);
    setInert(layer, false);
    syncInert();
  };
}

/** Is `layer` the top-most open modal? */
export function isTopModal(layer: HTMLElement): boolean {
  return modals[modals.length - 1] === layer;
}

// The attribute, not the IDL property: it is what CSS, the accessibility tree and tests all see.
function setInert(el: HTMLElement, on: boolean) {
  el.toggleAttribute("inert", on);
}

function syncInert() {
  if (appRoot) setInert(appRoot, modals.length > 0);
  modals.forEach((m, i) => setInert(m, i < modals.length - 1));
}

/** Forget every modal (tests). */
export function resetOverlays(): void {
  modals.splice(0).forEach((m) => setInert(m, false));
  syncInert();
}
