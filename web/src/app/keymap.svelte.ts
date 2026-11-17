// One keymap with a scope stack (spec §9.2 rule 4, fixes U5).
//
// The bottom scope is global. Overlays (Dialog, Menu, Popover) push a scope while they are open;
// a key is offered to the scopes from the top down, and a *blocking* scope stops it there whether
// or not it bound the key — so a global shortcut never acts behind a modal, and Escape closes only
// the top-most overlay.
//
// Components handle their own local keys (arrow keys inside a menu, Tab inside a dialog) with an
// element listener; the keymap listens on the window in the bubbling phase and ignores an event a
// component already handled (`defaultPrevented`).

export type KeyHandler = (event: KeyboardEvent) => boolean | void;

export interface BindOptions {
  /** Fire even when focus is in a text field. Escape always does. */
  inInputs?: boolean;
}

interface Binding {
  combo: string;
  handler: KeyHandler;
  inInputs: boolean;
}

interface Scope {
  name: string;
  blocking: boolean;
  bindings: Binding[];
}

export interface ScopeHandle {
  readonly name: string;
  bind(combo: string, handler: KeyHandler, options?: BindOptions): () => void;
  /** Remove this scope (and anything pushed above it that was not popped — a leak guard). */
  pop(): void;
}

const isMac = () => typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

/**
 * Canonical form of a combo: modifiers in the order Mod, Alt, Shift, then the key. `Mod` is
 * Ctrl, or Cmd on macOS. Letters are lower case; Space is `Space`. A printed symbol already
 * encodes Shift (`?` is Shift+/ on a US layout), so Shift is dropped for non-letter characters.
 */
export function normalizeCombo(combo: string): string {
  const parts = combo.split("+").map((p) => p.trim()).filter(Boolean);
  // "Mod++" — the key itself is a plus.
  if (combo.endsWith("++")) parts.push("+");
  const key = parts.pop() ?? "";
  const mods = new Set(parts.map((m) => m.toLowerCase()));
  return build(mods.has("mod") || mods.has("ctrl") || mods.has("cmd") || mods.has("meta"), mods.has("alt"), mods.has("shift"), key);
}

export function comboOf(event: KeyboardEvent): string {
  const mod = isMac() ? event.metaKey : event.ctrlKey;
  return build(mod, event.altKey, event.shiftKey, event.key);
}

function build(mod: boolean, alt: boolean, shift: boolean, rawKey: string): string {
  let key = rawKey === " " ? "Space" : rawKey;
  const single = [...key].length === 1;
  if (single) {
    const lower = key.toLowerCase();
    const letter = lower !== key.toUpperCase();
    key = lower;
    if (!letter) shift = false;
  } else if (key.length > 1) {
    key = key[0].toUpperCase() + key.slice(1);
  }
  return [mod && "Mod", alt && "Alt", shift && "Shift", key].filter(Boolean).join("+");
}

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
  if (target instanceof HTMLInputElement) {
    return !["button", "checkbox", "radio", "range", "reset", "submit", "color", "file", "image"].includes(target.type);
  }
  return false;
}

export class Keymap {
  #stack: Scope[] = [{ name: "global", blocking: false, bindings: [] }];
  /** How many blocking scopes are open — reactive, for UI that must know a modal is up. */
  blockingDepth = $state(0);

  /** Bind in the global scope. Returns the unbind function. */
  bind(combo: string, handler: KeyHandler, options: BindOptions = {}): () => void {
    return this.#bindIn(this.#stack[0], combo, handler, options);
  }

  /** Open a scope above everything currently open. Overlays are blocking. */
  push(name: string, { blocking = true }: { blocking?: boolean } = {}): ScopeHandle {
    const scope: Scope = { name, blocking, bindings: [] };
    this.#stack.push(scope);
    this.#sync();
    return {
      name,
      bind: (combo, handler, options = {}) => this.#bindIn(scope, combo, handler, options),
      pop: () => {
        const at = this.#stack.indexOf(scope);
        if (at > 0) this.#stack.splice(at);
        this.#sync();
      },
    };
  }

  /** The names of the open scopes, bottom first (for tests and debugging). */
  get scopes(): string[] {
    return this.#stack.map((s) => s.name);
  }

  /** Offer an event to the scopes, top-most first. Returns true when a binding handled it. */
  handle(event: KeyboardEvent): boolean {
    if (event.defaultPrevented || event.isComposing) return false;
    const combo = comboOf(event);
    const editable = isEditable(event.target);
    for (let i = this.#stack.length - 1; i >= 0; i--) {
      const scope = this.#stack[i];
      for (let b = scope.bindings.length - 1; b >= 0; b--) {
        const binding = scope.bindings[b];
        if (binding.combo !== combo) continue;
        if (editable && !binding.inInputs && combo !== "Escape") continue;
        if (binding.handler(event) !== false) {
          event.preventDefault();
          return true;
        }
      }
      if (scope.blocking) return false;
    }
    return false;
  }

  /** Listen on `target` (the window by default). Returns the detach function. */
  attach(target: Window | HTMLElement = window): () => void {
    const listener = (e: Event) => this.handle(e as KeyboardEvent);
    target.addEventListener("keydown", listener);
    return () => target.removeEventListener("keydown", listener);
  }

  /** Drop every scope and binding (tests). */
  reset(): void {
    this.#stack = [{ name: "global", blocking: false, bindings: [] }];
    this.#sync();
  }

  #bindIn(scope: Scope, combo: string, handler: KeyHandler, options: BindOptions): () => void {
    const binding: Binding = { combo: normalizeCombo(combo), handler, inInputs: options.inInputs ?? false };
    scope.bindings.push(binding);
    return () => {
      const at = scope.bindings.indexOf(binding);
      if (at >= 0) scope.bindings.splice(at, 1);
    };
  }

  #sync() {
    this.blockingDepth = this.#stack.filter((s) => s.blocking).length;
  }
}

/** The app's keymap. `main.ts` attaches it to the window. */
export const keymap = new Keymap();
