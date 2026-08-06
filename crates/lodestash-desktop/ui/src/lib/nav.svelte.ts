import type { Route, TabId } from "./routes";
import { emptyFilter, filterCount, DENSITIES, SORTS, type ContentFilter, type WebPrefs } from "./api";

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
  /**
   * B8c: the two independent axes the design keeps apart — **browse view** (Flat · Folders) ×
   * **row style** (Grid · List). The toolbar's three stops map onto them; both persist via
   * WebPrefs (config.json), so a library reopens the way you left it.
   */
  browse = $state<"flat" | "folders">("flat");
  rowStyle = $state<"grid" | "list">("grid");
  /** Folders view position: which source we are inside, and where in it. */
  source = $state<string | null>(null);
  dir = $state("");
  inspectorOpen = $state(true);
  /** Lead selection, as an asset id. Selection is view state: no worker writes it. */
  selected = $state<number | null>(null);
  /**
   * Bulk-bar multi-select (KANBAN B9d): ids picked with the card checkboxes. Deliberately apart
   * from `selected` — the lead drives single-click / Quick Look, while this is a working list the
   * bulk bar tags at once. It lives in the view store because it must survive re-renders: the
   * grid owns the checkboxes, but a selection outlives any one page's row set (and paging).
   */
  selectedIds = $state<number[]>([]);
  /**
   * Browse history for the pack browser (PACK-DIRECTORY-NAV §4): stacks of `dir` strings behind
   * the ← → buttons. Scoped to pack browsing — the generic Folders view breadcrumb jumps directly.
   */
  histBack = $state<string[]>([]);
  histFwd = $state<string[]>([]);
  /** The ⚑ popover's selection, and whether it is open. Transient — never persisted (design §3). */
  filter = $state<ContentFilter>(emptyFilter());
  filtersOpen = $state(false);
  /** Active pack filter, set by Packages' Browse → and shown as a clearable chip. */
  packageFilter = $state<string | null>(null);
  /** Active collection filter, set by Collections' Browse → and shown as a clearable chip. */
  collectionFilter = $state<{ id: number; name: string } | null>(null);
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

  /**
   * Navigate `dir` through the pack browser's history stacks (PACK-DIRECTORY-NAV §4). Pushes the
   * previous position onto `histBack`, clears the forward stack, then moves.
   */
  navDir(dir: string) {
    if (dir === this.dir) return;
    this.histBack = [...this.histBack, this.dir];
    this.histFwd = [];
    this.dir = dir;
  }

  back() {
    const prev = this.histBack.at(-1);
    if (prev === undefined) return;
    this.histBack = this.histBack.slice(0, -1);
    this.histFwd = [this.dir, ...this.histFwd];
    this.dir = prev;
  }

  fwd() {
    const next = this.histFwd[0];
    if (next === undefined) return;
    this.histFwd = this.histFwd.slice(1);
    this.histBack = [...this.histBack, this.dir];
    this.dir = next;
  }

  /**
   * The sidebar's Library row is the reset-to-everything affordance (UI-SPEC §0.2): it clears the
   * query, the facets, the ⚑ filter and the pack chip. Kept separate from `go("library")` because
   * Packages' Browse → also lands on Library and must keep the filter it just set.
   */
  goLibrary() {
    this.route = "library";
    this.search = "";
    this.tab = "all";
    this.status = "all";
    this.packageFilter = null;
    this.collectionFilter = null;
    this.source = null;
    this.dir = "";
    this.clearFilter();
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
   * Toggle one id in the bulk multi-select (B9d). Reassigns the array rather than mutating it in
   * place — `$state` tracks the property, so a screen deriving `selectedIds.includes(id)` from
   * the current array would otherwise not re-render.
   */
  toggleSelect(id: number) {
    this.selectedIds = this.selectedIds.includes(id)
      ? this.selectedIds.filter((x) => x !== id)
      : [...this.selectedIds, id];
  }

  /** Empty the bulk multi-select — the bulk bar's Clear, and after every bulk tag write. */
  clearSelection() {
    this.selectedIds = [];
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

  /**
   * Apply persisted prefs (WebPrefs) once at startup. Unknown values keep the current state —
   * a future config must never crash an older frontend, and vice versa.
   */
  hydrate(p: WebPrefs) {
    if (p.browse === "flat" || p.browse === "folders") this.browse = p.browse;
    if (p.row_style === "grid" || p.row_style === "list") this.rowStyle = p.row_style;
    if (DENSITIES.some((d) => d.id === p.density)) this.density = p.density as typeof this.density;
    if (SORTS.some((s) => s.id === p.sort_by)) this.sortBy = p.sort_by;
    if (typeof p.sort_asc === "boolean") this.sortAsc = p.sort_asc;
  }

  /** The slice of view state that persists (FEATURES §10). */
  snapshot(): WebPrefs {
    return {
      browse: this.browse,
      row_style: this.rowStyle,
      density: this.density,
      sort_by: this.sortBy,
      sort_asc: this.sortAsc,
    };
  }
}

export const view = new ViewState();
