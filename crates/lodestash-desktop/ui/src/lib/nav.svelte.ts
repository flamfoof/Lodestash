import type { Route, TabId } from "./routes";

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
  /** Grid of cards, or the dense table (UI-SPEC §1.3 view toggle). */
  viewMode = $state<"grid" | "list">("grid");
  inspectorOpen = $state(true);
  /** Lead selection, as an asset id. Selection is view state: no worker writes it. */
  selected = $state<number | null>(null);

  go(route: Route) {
    this.route = route;
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
