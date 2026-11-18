// Selection: which asset the inspector describes, and which assets a bulk action acts on.
//
// Two facts, kept apart as UI-SPEC §0.4 keeps them:
//   - the **lead** — one asset, the gold ring, what the inspector shows; arrows and a click move it,
//     and keyboard focus follows it;
//   - the **pick set** — what a bulk action acts on; the pick box, Ctrl/Cmd-click, Shift with a
//     click or an arrow, and Ctrl/Cmd-A change it. Moving the lead never changes it.
// The class holds ids only; the owner of the page's assets resolves them, so a selection can never
// hold a stale copy of an asset.

import { SvelteSet } from "svelte/reactivity";

export class Selection {
  lead = $state<number | null>(null);
  readonly picks = new SvelteSet<number>();
  /** Where a Shift range starts: the last asset chosen without Shift. */
  #anchor: number | null = null;

  /** A click: `id` becomes the lead; clicking the lead again deselects it (UI-SPEC §2). */
  open(id: number): void {
    this.lead = this.lead === id ? null : id;
    this.#anchor = id;
  }

  /** Arrow keys, Enter and "Open": `id` is the lead, whatever it was. */
  moveTo(id: number): void {
    this.lead = id;
    this.#anchor = id;
  }

  /** The pick box and Ctrl/Cmd-click: put `id` in the pick set, or take it out. */
  toggle(id: number): void {
    if (!this.picks.delete(id)) this.picks.add(id);
    this.#anchor = id;
  }

  /**
   * Shift with a click or an arrow: the pick set becomes the run from the anchor to `id` in
   * `order` (the page's ids as shown), and the lead moves to `id`.
   */
  extendTo(id: number, order: readonly number[]): void {
    const to = order.indexOf(id);
    if (to < 0) return;
    const start = order.indexOf(this.#anchor ?? this.lead ?? id);
    const from = start < 0 ? to : start;
    this.picks.clear();
    for (let i = Math.min(from, to); i <= Math.max(from, to); i++) this.picks.add(order[i]);
    this.#anchor = order[from];
    this.lead = id;
  }

  /** Ctrl/Cmd-A: every asset on the current page (UI-SPEC §0.4 — the page, not the whole filter). */
  pickAll(order: readonly number[]): void {
    for (const id of order) this.picks.add(id);
  }

  /** Escape: drop the pick set; with none, the lead. False when there was nothing to clear. */
  clear(): boolean {
    if (this.picks.size > 0) {
      this.picks.clear();
      return true;
    }
    if (this.lead === null) return false;
    this.lead = null;
    return true;
  }

  /** Forget ids that are no longer shown: a selection follows the filter. */
  prune(order: readonly number[]): void {
    const shown = new Set(order);
    for (const id of [...this.picks]) if (!shown.has(id)) this.picks.delete(id);
    if (this.lead !== null && !shown.has(this.lead)) this.lead = null;
    if (this.#anchor !== null && !shown.has(this.#anchor)) this.#anchor = null;
  }

  /**
   * What an action acts on. Invoked on an asset (`id`): the whole pick set when that asset is in
   * it, otherwise that asset alone. Invoked with no asset (a key): the pick set, or with none, the
   * lead.
   */
  targets(id?: number): number[] {
    if (id !== undefined) return this.picks.has(id) ? [...this.picks] : [id];
    if (this.picks.size > 0) return [...this.picks];
    return this.lead === null ? [] : [this.lead];
  }

  /** Is anything picked? The pick boxes stay visible while it is. */
  get picking(): boolean {
    return this.picks.size > 0;
  }
}
