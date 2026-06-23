// Rasterize lodestash-icon.svg → PNG ladder using sharp.
//   npm i sharp && node gen.js
// Then build favicon.ico (see README.md).
const sharp = require('sharp');
const fs = require('fs');
const path = require('path');

const DIR = __dirname;
const master = fs.readFileSync(path.join(DIR, 'lodestash-icon.svg'));
const pngDir = path.join(DIR, 'png');
fs.mkdirSync(pngDir, { recursive: true });

// Render the SVG at high density, then resize down → crisp at every size.
const render = (size, outfile) =>
  sharp(master, { density: 144 })
    .resize(size, size, { fit: 'contain', background: { r: 0, g: 0, b: 0, alpha: 0 } })
    .png({ compressionLevel: 9 })
    .toFile(outfile);

(async () => {
  for (const s of [16, 32, 48, 64, 128, 256, 512, 1024]) await render(s, path.join(pngDir, `icon-${s}.png`));
  await render(192, path.join(DIR, 'icon-192.png'));
  await render(512, path.join(DIR, 'icon-512.png'));
  await render(180, path.join(DIR, 'apple-touch-icon.png'));
  console.log('PNG generation complete.');
})().catch(e => { console.error(e); process.exit(1); });
