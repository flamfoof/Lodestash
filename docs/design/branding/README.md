# Lodestash — Brand & App Icons

The Lodestash mark: the **lodestone "guiding star"** (gold) on a dark JRPG tile — matching the design
system ([`../DESIGN-GUIDELINES.md`](../DESIGN-GUIDELINES.md)).

## Files

| File | Use |
|---|---|
| `lodestash-icon.svg` | **Master** app mark (512, full detail: glow + ring). Source of truth. |
| `favicon.svg` | Simplified mark (bolder star, no fine detail) for crisp small sizes. |
| `favicon.ico` | Multi-res ICO (16/32/48/64) for browsers & Windows. |
| `apple-touch-icon.png` | 180×180 for iOS home-screen. |
| `icon-192.png`, `icon-512.png` | PWA / web manifest. |
| `png/icon-{16..1024}.png` | Full size ladder for app/store/installer use. |
| `site.webmanifest` | Web manifest referencing the PNG icons. |

## Web usage

```html
<link rel="icon" href="favicon.ico" sizes="48x48">
<link rel="icon" href="favicon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="apple-touch-icon.png">
<link rel="manifest" href="site.webmanifest">
<meta name="theme-color" content="#E7B765">
```

## Regenerating (after editing `lodestash-icon.svg`)

Rasterized with **sharp** (SVG→PNG) and **Pillow** (multi-res ICO):

```bash
# PNGs (Node + sharp): render at high density, resize down for crisp output
node gen.js          # ladder 16..1024 + icon-192/512 + apple-touch-icon (180)

# favicon.ico (Python + Pillow)
python -c "from PIL import Image; Image.open('png/icon-256.png').convert('RGBA').save('favicon.ico', sizes=[(16,16),(32,32),(48,48),(64,64)])"
```

> `gen.js` reads `lodestash-icon.svg` and writes the PNGs. Colors are the design tokens:
> tile `#20283A→#0B0E14`, star/gold `#E7B765` (`--ls-gold`), background `#0E1117`, theme `#E7B765`.
