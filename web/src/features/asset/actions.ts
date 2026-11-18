// Asset actions — the one place an asset intent is defined (spec §9.2 rule 6).
//
// The card, the list row, the context menu, the inspector and the keymap all ask this module what
// an action is called, whether it can run, and what intent it produces; none of them builds an
// intent itself. `AssetIntent` is branded with a symbol this module does not export, so no other
// module can construct one — a second definition of "stage" is a type error, not a review comment.
//
// An intent says what the person asked for. Whoever owns the data (the specimen today, the
// mutation layer in plan P6b) carries it out; nothing here touches the catalog.

import { isControl, type Keymap } from "../../app/keymap.svelte";
import type { IconName, MenuEntry } from "../../ui";
import type { AssetSummary, ProjectRef } from "./model";
import { userStatus } from "./present";

export type AssetActionId =
  | "open"
  | "quick-look"
  | "play"
  | "reveal"
  | "open-default"
  | "copy-name"
  | "copy-path"
  | "favorite"
  | "stage"
  | "import"
  | "export"
  | "collect"
  | "edit-tags"
  | "identify"
  | "find-similar"
  | "trash"
  | "restore";

declare const brand: unique symbol;

export interface AssetIntent {
  readonly action: AssetActionId;
  /** The assets it acts on, in the order they were given. Never empty. */
  readonly ids: readonly number[];
  /** `favorite` and `stage`: the state to set (a toggle resolved against the targets). */
  readonly on?: boolean;
  readonly [brand]: true;
}

export interface ActionContext {
  /** What the action acts on: one asset, or the pick set. */
  targets: readonly AssetSummary[];
  /** The active import target; null when no project is connected. */
  project: ProjectRef | null;
}

export interface ActionDef {
  label(ctx: ActionContext): string;
  icon?: IconName;
  /** As shown in a menu ("Space", "Del"). */
  shortcut?: string;
  danger?: boolean;
  /** Why it cannot run now — a disabled menu item's tooltip. Undefined when it can. */
  unavailable?(ctx: ActionContext): string | undefined;
  /** The state a toggle sets. */
  on?(ctx: ActionContext): boolean;
}

const isMac = () => typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
const one = (ctx: ActionContext) => (ctx.targets.length === 1 ? undefined : "Select a single asset");
const every = (ctx: ActionContext, flag: "favorite" | "staged") => ctx.targets.every((a) => a[flag]);
const unidentified = (a: AssetSummary) => userStatus(a.status) === "unidentified";
const plural = (ctx: ActionContext, singular: string) => (ctx.targets.length > 1 ? `${singular}s` : singular);

export const ACTIONS: Record<AssetActionId, ActionDef> = {
  open: { label: () => "Open", shortcut: "Enter", unavailable: one },
  "quick-look": { label: () => "Quick Look", icon: "action-fullsize", shortcut: "Space" },
  play: {
    label: () => "Play",
    icon: "play",
    unavailable: (ctx) => one(ctx) ?? (ctx.targets[0].media_type === "audio" ? undefined : "Only sounds play"),
  },
  reveal: { label: () => (isMac() ? "Reveal in Finder" : "Reveal in Explorer"), icon: "folder-open", unavailable: one },
  "open-default": { label: () => "Open with default app", unavailable: one },
  "copy-name": { label: (ctx) => plural(ctx, "Copy name"), icon: "action-copy" },
  "copy-path": { label: (ctx) => plural(ctx, "Copy path") },
  // A toggle over several assets turns them all on unless every one already is (UI-SPEC §0.3).
  favorite: {
    label: (ctx) => (every(ctx, "favorite") ? "Remove from Favorites" : "Add to Favorites"),
    icon: "nav-favorites",
    shortcut: "F",
    on: (ctx) => !every(ctx, "favorite"),
  },
  stage: {
    label: (ctx) => (every(ctx, "staged") ? "Unstage" : "Stage"),
    icon: "action-stage",
    on: (ctx) => !every(ctx, "staged"),
  },
  import: {
    label: (ctx) => (ctx.project ? `Import into ${ctx.project.name}…` : "Import…"),
    icon: "import",
    // DESIGN-GUIDELINES §7: engine actions gray out, with the reason, when no project is active.
    unavailable: (ctx) => (ctx.project ? undefined : "No project is connected — Export to folder works without one"),
  },
  export: { label: () => "Export to folder…", icon: "download" },
  collect: { label: () => "Add to collection…", icon: "nav-collections" },
  "edit-tags": { label: () => "Edit tags…", icon: "tag-add" },
  identify: {
    label: (ctx) => (ctx.targets.every(unidentified) ? "Identify" : "Re-identify"),
    icon: "action-ai-sieve",
  },
  "find-similar": {
    label: () => "Find similar",
    icon: "similar",
    unavailable: (ctx) => one(ctx) ?? (unidentified(ctx.targets[0]) ? "Unidentified assets cannot be compared yet" : undefined),
  },
  trash: { label: () => "Move to Trash", icon: "nav-trash", shortcut: "Del", danger: true },
  restore: { label: () => "Restore", icon: "action-regenerate" },
};

/** Why `id` cannot run for `ctx`, or undefined when it can. */
export function whyNot(id: AssetActionId, ctx: ActionContext): string | undefined {
  if (ctx.targets.length === 0) return "Nothing is selected";
  return ACTIONS[id].unavailable?.(ctx);
}

/** The intent `id` produces for `ctx` — null when the action cannot run. */
export function intentFor(id: AssetActionId, ctx: ActionContext): AssetIntent | null {
  if (whyNot(id, ctx) !== undefined) return null;
  const on = ACTIONS[id].on?.(ctx);
  const intent = { action: id, ids: Object.freeze(ctx.targets.map((a) => a.id)), ...(on === undefined ? {} : { on }) };
  return Object.freeze(intent) as AssetIntent;
}

/** The context menu, in DESIGN-GUIDELINES §7's order. `null` is a separator. */
export const MENU: readonly (AssetActionId | null)[] = [
  "open",
  "quick-look",
  null,
  "reveal",
  "open-default",
  "copy-name",
  "copy-path",
  null,
  "favorite",
  "stage",
  "import",
  "export",
  "collect",
  "edit-tags",
  null,
  "identify",
  "find-similar",
  null,
  "trash",
];

/** The menu's entries for `ctx`: the same menu for one asset and for many. */
export function menuFor(ctx: ActionContext): MenuEntry[] {
  return MENU.map((id) => {
    if (id === null) return { separator: true };
    const def = ACTIONS[id];
    const hint = whyNot(id, ctx);
    return { id, label: def.label(ctx), icon: def.icon, shortcut: def.shortcut, danger: def.danger, disabled: hint !== undefined, hint };
  });
}

/** Is `id` an action id? A menu hands back the id it was given as a plain string. */
export function isActionId(id: string): id is AssetActionId {
  return Object.hasOwn(ACTIONS, id);
}

/** What the keys act on, read at the moment a key is pressed. */
export interface KeySource {
  /** The asset the inspector describes. */
  lead(): AssetSummary | null;
  /** The pick set, or with none, the lead. */
  selection(): readonly AssetSummary[];
  project(): ProjectRef | null;
}

/**
 * Bind the asset keys of UI-SPEC §0.4 in the keymap's global scope, so an open overlay's blocking
 * scope stops them: Space → Quick Look (the lead), F → favourite (the pick set, else the lead),
 * Delete → Move to Trash (the lead only — a key must not bin a pick set no bar announces).
 * Returns the unbind function.
 */
export function bindAssetKeys(keymap: Keymap, source: KeySource, emit: (intent: AssetIntent) => void): () => void {
  const run = (id: AssetActionId, targets: readonly AssetSummary[]) => {
    const intent = intentFor(id, { targets, project: source.project() });
    if (!intent) return false;
    emit(intent);
    return true;
  };
  const lead = () => {
    const asset = source.lead();
    return asset ? [asset] : [];
  };
  const unbind = [
    // A focused button or link takes Space itself.
    keymap.bind("Space", (event) => !isControl(event.target) && run("quick-look", lead())),
    keymap.bind("f", () => run("favorite", source.selection())),
    keymap.bind("Delete", () => run("trash", lead())),
  ];
  return () => unbind.forEach((off) => off());
}
