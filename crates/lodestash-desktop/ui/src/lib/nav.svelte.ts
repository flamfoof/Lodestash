import type { Route, TabId } from "./routes";
import { emptyFilter, filterCount, type ContentFilter } from "./api";

/**
 * Frontend-owned view state (plan Phase B, "State ownership"): route, tab, and search
 * live here because no background thread ever writes them. Anything a worker mutates —
 * entries, jobs, config — stays behind the Rust `Engine` and arrives via `lodestash://changed`.
 */
class ViewState {
  route = $state<Route>("library");
  tab = $state<TabId>("all");
  search = $state("");
  searchMode = $state<"tags" | "ai">("tags");
  status = $state("all");
  sortBy = $state("name");
  sortAsc = $state(true);
  density = $state<"compact" | "cozy" | "large">("cozy");
  /** Grid of cards, the dense table, or the directory browser (DESIGN-GUIDELINES §7). */
  viewMode = $state<"grid" | "list" | "folders">("grid");
  /** Folders view position: which source we are inside, and where in it. */
  source = $state<string | null>(null);
  dir = $state("");
  inspectorOpen = $state(true);
  /** Lead selection, as an asset id. Selection is view state: no worker writes it. */
  selected = $state<number | null>(null);
  /** The ⚑ popover's selection, and whether it is open. Transient — never persisted (design §3). */
  filter = $state<ContentFilter>(emptyFilter());
  filtersOpen = $state(false);
  /**
   * Bumped after every catalog write. Screens depend on it, so a mutation re-runs their query.
   *
   * Crude on purpose: there is no change notification until the pump thread (B2b) exists, and the
   * backend already re-reads the library after a write. A counter re-asks the question rather than
   * guessing the answer — the alternative, patching rows locally to match what we think the write
   * did, is how a UI ends up disagreeing with its own database.
   */
  dataVersion = $state(0);

  go(route: Route) {
    this.route = route;
  }

  /** How many chips are on — the gold badge on the ⚑ button. */
  get filterCount() {
    return filterCount(this.filter);
  }

  /**
   * Toggle one chip in one group.
   *
   * Reassigns the whole filter rather than mutating the array in place: `$state` tracks the
   * property, and a screen deriving its query from `view.filter` would otherwise keep the stale
   * one and quietly not re-query.
   */
  toggleFacet(group: keyof ContentFilter, id: string) {
    const on = this.filter[group];
    this.filter = {
      ...this.filter,
      [group]: on.includes(id) ? on.filter((x) => x !== id) : [...on, id],
    };
  }

  clearFilter() {
    this.filter = emptyFilter();
  }

  /**
   * Set the query, and follow it with the sort.
   *
   * Typing a query switches to Relevance so BM25 order is what you see; clearing it puts Name
   * back. Both moves are skipped if the user has chosen some *other* sort — picking Size and
   * then searching should not silently re-sort the results, and this only ever swaps between
   * the two orders it owns.
   */
  setSearch(text: string) {
    const had = this.search.trim().length > 0;
    const has = text.trim().length > 0;
    this.search = text;
    if (has && !had && this.sortBy === "name") this.sortBy = "relevance";
    else if (!has && had && this.sortBy === "relevance") this.sortBy = "name";
  }
}

export const view = new ViewState();
