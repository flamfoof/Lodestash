import type { LibraryStatus, SourceSummary } from "./api";

/**
 * Source availability, readable from anywhere (UI-SPEC §11.1).
 *
 * A card's `file unavailable` plate, a row's inert `⤢`, Quick Look's *"this file isn't reachable"*
 * panel and the staging tray's partial-import refusal are all the **same one fact** — the state of
 * the source that row belongs to. `GridItem.source_id` carries the key; this holds the map.
 *
 * A module store rather than a prop: `status` lives in App, but the components that need this are
 * three and four levels down and one of them (`Card`) renders 8,000 times. Threading a whole
 * `LibraryStatus` through the grid to read one boolean per card is plumbing, not design.
 *
 * Empty means "nothing known to be gone", which is both the startup state and the common one —
 * `unavailable()` answering `false` while the first health check is still in flight is deliberate
 * (a false plate flashing on every open would be worse than the gap it closes).
 */
class SourceHealth {
  by_id = $state<Record<number, SourceSummary>>({});

  /** Re-read from the status the app just fetched. Called wherever `status` is assigned. */
  sync(status: LibraryStatus | null) {
    const next: Record<number, SourceSummary> = {};
    for (const s of status?.sources ?? []) next[s.id] = s;
    this.by_id = next;
  }

  get(id: number): SourceSummary | undefined {
    return this.by_id[id];
  }

  unavailable(id: number): boolean {
    return this.by_id[id]?.state === "unavailable";
  }

  /** The gone sources, for the Library banner and the Settings relink rows. */
  get gone(): SourceSummary[] {
    return Object.values(this.by_id).filter((s) => s.state === "unavailable");
  }
}

export const sources = new SourceHealth();
