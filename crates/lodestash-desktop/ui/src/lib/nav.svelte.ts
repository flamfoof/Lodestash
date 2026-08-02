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
  inspectorOpen = $state(true);

  go(route: Route) {
    this.route = route;
  }
}

export const view = new ViewState();
