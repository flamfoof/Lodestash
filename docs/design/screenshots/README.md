# Prototype screenshots — partial mirror

Captures of the **Lodestash Prototype**, mirrored from the Claude Design project
`f1fd4bce-f7bc-4b33-b099-d74b78f31998`, path `screenshots/`.

## Read these last

Every shot here has a **Unity nav row** and **no Favorites**, which places all of them *before*
`../Lodestash Prototype.dc.html`, which has Favorites and no Unity row. The precedence order is in
`../README.md`: `FEATURES.md` > prototype > these. Never take a label, count or nav order from a
screenshot when either of the other two disagrees.

## The numbering is frames, not revisions

`NN-name` is **frame NN of the `name` sequence** — a scripted flow captured step by step. It is not a
version number. Proof: `01-grid`, `01-pin`, `01-remove` and `01-rm` are byte-identical (same md5),
because frame 1 of all four flows is the same untouched library screen. Three of those four are
deleted here; `01-grid.jpg` stands in for all of them.

So **the last frame of a sequence is the interesting one** — it holds the end state the flow was
capturing. `05-grid.jpg` (the terminal frame of the grid flow) shows a pack-scoped library with the
`Add all → N to project` pill and the §5.6 duplicate-on-import toast; `01-grid.jpg` shows none of
that.

Two sequences were captured at different times and disagree on the Unity nav label — `01-unity.jpg`
reads **Unity Libraries**, `05-grid.jpg` reads **Unity Assets**. `FEATURES.md` §0.3 settles it:
**Unity Assets**.

## What is here, and what is not

Mirrored: `01-filterpanel`, `01-grid`, `01-pin2`, `01-settings`, `01-settings-new`, `01-unity`,
`05-grid`.

The remote holds ~60 more (the `02-`…`05-` frames of these sequences, plus `u3`–`u8`, `diag`, `fp2`,
`onboarding-over-library`, `settings-lower`, `glyph-catalog`). They were **not** mirrored: the
DesignSync `get_file` transport returns base64 inline for files under ~50 KB, so a bulk pull spends
the reader's context on image bytes. Pull individually, by name, when a specific screen is being
built — and prefer the **highest-numbered frame** of the sequence you care about.

Files are `.jpg` despite the remote `.png` names; the stored bytes are JPEG (`ffd8ffe0`).
