/**
 * App-level toasts (KANBAN B10c / I5). A tiny module-scoped `$state` store — the same shared
 * instance is imported by App.svelte, which renders the host. DESIGN-GUIDELINES §7: bottom-right,
 * `--ls-overlay`, 1px accent left-border by type (ok → success, err → danger).
 */
export type ToastKind = "ok" | "err";

export type Toast = {
  id: number;
  msg: string;
  kind: ToastKind;
};

/** Auto-dismiss after this long (B10c: "~3.5s"). */
const TOAST_MS = 3500;

let nextId = 1;
export const toasts = $state<Toast[]>([]);

/** Push a toast; it auto-dismisses after ~3.5s or on explicit `dismiss`. */
export function push(msg: string, kind: ToastKind = "ok"): void {
  const id = nextId++;
  toasts.push({ id, msg, kind });
  setTimeout(() => dismiss(id), TOAST_MS);
}

/** Remove a toast by id (the ✕ button; auto-dismiss calls this too — idempotent). */
export function dismiss(id: number): void {
  const i = toasts.findIndex((t) => t.id === id);
  if (i >= 0) toasts.splice(i, 1);
}
