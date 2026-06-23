# Lodestash — User Guide (Getting Started)

Lodestash is your **local asset hub**: it catalogs everything you own, lets you **search by tag or by
plain-language description**, preview it, and **pull just what you need into Unity/Unreal** — without
ever importing whole asset packs into your project.

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

## Step 6 — Bring it into your engine (the bridge)

With your Unity (or Unreal) project open and the Lodestash plugin panel active:

1. Search/preview inside the panel (same catalog).
2. **Select one or more assets** — even from different packs at once.
3. Click **Import**.

Lodestash pulls in **only those assets plus their dependencies** (e.g. the beaver mesh + its materials
and textures) into your project — **never the whole pack**. That keeps your project lean and clean.

You can also do this from **Claude** (or another AI client) via the built-in MCP connection.

---

## Tips

- **Re-run identification** whenever you download new packs (or set it to auto-run on new assets).
- **Non-content assets** (tools/scripts) are hidden by default; toggle them on if you ever need them.
- **Owned but not downloaded?** Optional Unity *account sync* can show packs you own but haven't
  downloaded as greyed-out entries — download them in Unity's My Assets to make them usable.
- **Privacy:** everything runs locally. Cloud AI is opt-in only and never required.
- **Pick your models:** [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md) tells you exactly what to install
  for your GPU.
