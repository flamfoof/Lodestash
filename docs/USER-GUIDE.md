# Lodestash — User Guide (Getting Started)

Lodestash is your **local asset hub**: it catalogs everything you own, lets you **search by tag or by
plain-language description**, preview it, and **pull just what you need into Unity** — without
ever importing whole asset packs into your project. (Unreal support is planned for a later phase.)

> **Status note (updated 2026-08, see [`KANBAN-SVELTE.md`](KANBAN-SVELTE.md) for the live board):**
> Steps 1–5 work end-to-end today — auto-discovery of the Asset Store cache, cache watching,
> identification (assets flip to *Identified* as tagging runs), and AI search are all live.
> **Step 6 (the Unity bridge) and the MCP `import_to_project` path are engine-project work, not yet
> runnable** — the REST + MCP servers ship with the app and 10 agent operations are live, but the
> Unity plugin (editor) half and the import round-trip still need a Unity install. The board tracks
> the exact deltas (🟢 done · 🟡 partial · 🔵 engine-project).

Follow these steps in order. **Step 3 (run identification) matters most — do it before you search.**

---

## Step 1 — Add your asset libraries

On first launch, Lodestash auto-finds your **Unity Asset Store download cache** and offers to add it.
Then add anything else:

- **Unity Asset Store** — auto-discovered (no setup).
- **Humble Bundle / Itch / other sites** — unzip each pack into a folder, then in Lodestash click
  **Add Library → Folder** and point it at that folder (or a parent folder holding many of them).
- **Loose `.unitypackage` files** — just add the folder they live in.

Lodestash scans them, makes thumbnails, removes duplicates, and **hides non-content junk** (scripts,
DLLs, editor tools, shaders) automatically.

---

## Step 2 — Download from the Asset Store, but **don't import**

This is the key habit. You do **not** import asset packs into a Unity project anymore — you let
Lodestash be the bridge:

1. In Unity, open **Window → Package Manager → My Assets**.
2. Find a pack you own and click **Download** — **but do NOT click Import.**
   - "Download" just saves the `.unitypackage` into Unity's cache.
   - "Import" would dump the *entire* pack into your project — exactly what we're avoiding.
3. That's it. Lodestash detects the downloaded pack automatically (it watches the cache) and adds it to
   your library — **browse-only at first, nothing unpacked into any project.**

> 💡 You can download dozens of packs this way. They live in one place; Lodestash indexes them all and
> later imports only the individual assets you actually pick.

---

## Step 3 — ⚠️ Run the Identification check **first**

**Before you try to search or access your assets, run identification on everything.** Freshly added
assets start as **Unidentified** — they have no AI tags or embeddings yet, so **"Search with AI" won't
find them** until this finishes.

1. Open **Identify mode**.
2. Click **Run identification on all Unidentified assets**.
3. Let it work through the list. Assets flip from **Unidentified → Identified** as they finish, and move
   into your searchable **Library**.

- This is a **one-time** cost per asset (results are cached; it won't redo them).
- A large library can take a while — it's fine to leave it running (overnight for huge collections).
- Speed depends on your hardware. Pick the right AI models for your GPU first — see
  [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md). No GPU? It still works on CPU, just slower.

> ✅ **Rule of thumb:** *Identify before you search.* If a search returns nothing, check whether those
> assets are still **Unidentified**.

---

## Step 4 — Search

Pick a **category** (Models / Sounds / Images), then choose how to search:

- **Search by Tags** — exact, fast, deterministic. Good when you know the keyword (`rock`, `pickup`).
- **Search with AI** — describe what you want in plain English. Examples:
  - *"a pickup sound for rocks"* → Sounds
  - *"a beaver"*, *"something with wings"*, *"a fruit model"* → Models

You'll get ranked matches pulled from across all your packs — or nothing, if you don't own anything
close.

---

## Step 5 — Preview

Click any result to **view the 3D model** (orbit around it), **play the audio** (with a waveform), or
**view the image/texture**. Audition several before deciding.

---

## Step 6 — Bring it into Unity (the bridge)

> ⚠️ **Not yet runnable** — this step is engine-project work (KANBAN-SVELTE 🔵): the REST + MCP
> servers ship with the app, but the Unity plugin and the import round-trip aren't built yet. The
> flow below describes the target behaviour.

With your Unity project open and the Lodestash plugin panel active (opening the panel **registers**
your project with Lodestash, so it knows where "into Unity" means):

1. Search/preview inside the panel (same catalog).
2. **Select one or more assets** — even from different packs at once. *(Tip: collect candidates onto
   the **staging tray** as you browse, then import the whole shortlist at once — Step "Stage" below.)*
3. Click **Import**.

Lodestash pulls in **only those assets plus their dependencies** (e.g. the beaver mesh + its materials
and textures) into your project — **never the whole pack**. That keeps your project lean and clean. If
you've imported an asset here before, Lodestash flags it **"already imported"** rather than duplicating
it, and shows its **license/attribution** so credits travel with the asset.

### …or ask Claude to do it (the MCP path) — ⚠️ not yet runnable

With the same panel open, you can tell **Claude** (or Claude Code, Cursor, any MCP client): *"find a
low-poly beaver and import it into my project under `Wildlife/`."* Claude searches the catalog, shows
you previews, and calls `import_to_project` — the asset lands in your **open** editor through the same
import path the panel uses (Lodestash hands the request to the running plugin, which imports it on
Unity's main thread). If no editor panel is open, Claude instead **exports** the asset to a folder you
name. Either way you never import a whole pack.

> **Stage, then import.** The staging tray is shared — assets you (or Claude) stage show up in the GUI,
> the Unity panel, and over MCP alike. Build a shortlist, review it, then import the keepers together.

---

## Tips

- **Re-run identification** whenever you download new packs (or set it to auto-run on new assets).
- **Non-content assets** (tools/scripts) are hidden by default; toggle them on if you ever need them.
- **Provenance & licenses:** each asset's detail panel shows which packs it came from, where you've
  imported it, and its **license/attribution** — handy when crediting bought assets in a shipped game.
- **Owned but not downloaded?** Optional Unity *account sync* can show packs you own but haven't
  downloaded as greyed-out entries — download them in Unity's My Assets to make them usable.
- **Privacy:** everything runs locally. Cloud AI is opt-in only and never required.
- **Pick your models:** [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md) tells you exactly what to install
  for your GPU.
