/**
 * App-level toasts (KANBAN B10c / I5, board D8). A tiny module-scoped `$state` store — the same
 * shared instance is imported by App.svelte, which renders the host. DESIGN-GUIDELINES §7:
 * bottom-right, `--ls-overlay`, 1px accent left-border by type (ok → success, err → danger).
 *
 * D8: a toast carries **one** action slot. `Undo` when the thing that just happened is reversible,
 * `Show` when the useful next move is to look at what failed — never both, never three. The moment
 * someone wants to undo is the second after they acted, looking at the thing that told them it
 * happened; the toolbar's ↶ is the slower path, not the only one.
 */
export type ToastKind = "ok" | "err" | "warn" | "info";

/** The single action slot. `run` may be async; the toast dismisses as soon as it is invoked. */
export type ToastAction = { label: string; run: () => void | Promise<void> };

export type Toast = {
  id: number;
  msg: string;
  kind: ToastKind;
  action?: ToastAction;
  /** Wall-clock ms at which this toast should go. Pushed forward while the stack is frozen. */
  expiresAt: number;
};

/** A toast you can act on stays twice as long — time to read it, act, and still get out of the way. */
const PLAIN_MS = 3500;
const ACTION_MS = 6000;
/** Four at once is a wall. The oldest goes and the count says so. */
const MAX = 3;

let nextId = 1;
export const toasts = $state<Toast[]>([]);
/** How many were dropped off the top while the stack was full — rendered as "+N more". */
export const overflow = $state({ count: 0 });

let frozen = false;
let frozenAt = 0;
let timer: ReturnType<typeof setInterval> | null = null;

/**
 * One sweep for the whole stack rather than a timer per toast: freezing then has to adjust a single
 * clock instead of cancelling and re-arming N of them, and a toast can never outlive its own timer.
 */
function sweep() {
  if (!frozen) {
    const now = Date.now();
    for (let i = toasts.length - 1; i >= 0; i--) {
      if (toasts[i].expiresAt <= now) toasts.splice(i, 1);
    }
  }
  if (toasts.length === 0) {
    if (timer) clearInterval(timer);
    timer = null;
    overflow.count = 0;
  }
}

/** Push a toast; it auto-dismisses, or on explicit `dismiss`. */
export function push(msg: string, kind: ToastKind = "ok", action?: ToastAction): void {
  const id = nextId++;
  toasts.push({ id, msg, kind, action, expiresAt: Date.now() + (action ? ACTION_MS : PLAIN_MS) });
  while (toasts.length > MAX) {
    toasts.shift();
    overflow.count += 1;
  }
  if (!timer) timer = setInterval(sweep, 250);
}

/** Remove a toast by id (the ✕ button; the sweep calls this too — idempotent). */
export function dismiss(id: number): void {
  const i = toasts.findIndex((t) => t.id === id);
  if (i >= 0) toasts.splice(i, 1);
  sweep();
}

/**
 * Hovering or focusing the stack freezes every timer in it — reaching for Undo must not be a race
 * against the toast that offered it. Thawing gives back exactly the time that was held.
 */
export function freeze(): void {
  if (frozen) return;
  frozen = true;
  frozenAt = Date.now();
}

export function thaw(): void {
  if (!frozen) return;
  frozen = false;
  const held = Date.now() - frozenAt;
  for (const t of toasts) t.expiresAt += held;
}

/** Fire a toast's action and take the toast away — the outcome speaks for itself in its own toast. */
export function act(id: number): void {
  const t = toasts.find((x) => x.id === id);
  if (!t?.action) return;
  dismiss(id);
  void t.action.run();
}
