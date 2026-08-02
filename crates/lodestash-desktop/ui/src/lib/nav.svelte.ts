import type { Route, TabId } from "./routes";

/**
 * Frontend-owned view state (plan Phase B, "State ownership"): route, tab, and search
 * live here because no background thread ever writes them. Anything a worker mutates —
 * entries, jobs, config — stays behind the Rust `Engine` and arrives via `ls://changed`.
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
}

export const view = new ViewState();
