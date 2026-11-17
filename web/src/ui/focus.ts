// Focus helpers shared by Dialog, Menu and Popover.

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  'input:not([disabled]):not([type="hidden"])',
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
  '[contenteditable="true"]',
].join(",");

/** The tabbable elements inside `root`, in document order. */
export function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => {
    if (el.closest("[inert],[hidden]")) return false;
    const cs = getComputedStyle(el);
    return cs.display !== "none" && cs.visibility !== "hidden";
  });
}

/**
 * Keep Tab and Shift+Tab inside `root`: from the last tabbable Tab wraps to the first, from the
 * first Shift+Tab wraps to the last. Returns true when it moved focus.
 */
export function trapTab(event: KeyboardEvent, root: HTMLElement): boolean {
  if (event.key !== "Tab") return false;
  const items = focusables(root);
  if (items.length === 0) {
    event.preventDefault();
    root.focus();
    return true;
  }
  const first = items[0];
  const last = items[items.length - 1];
  const active = document.activeElement;
  const inside = active instanceof HTMLElement && root.contains(active) && active !== root;
  if (event.shiftKey && (!inside || active === first)) {
    event.preventDefault();
    last.focus();
    return true;
  }
  if (!event.shiftKey && (!inside || active === last)) {
    event.preventDefault();
    first.focus();
    return true;
  }
  return false;
}

/** Focus `el` if it is still in the document and can take focus. */
export function restoreFocus(el: Element | null): void {
  if (el instanceof HTMLElement && el.isConnected && !el.closest("[inert]")) el.focus();
}
