// Build the static Vercel output in ./public from web/. Every run starts from an
// empty directory, so deleted or renamed pages can never linger in a deployment.
const fs = require('node:fs');
const path = require('node:path');

// The published site: one page, its two assets and the self-hosted fonts
// (with their licence). Keep in sync with web/index.html and web/site.css.
const PAGES = ['index.html'];
const ASSETS = ['site.css', 'site.js'];
const FONTS = 'fonts';

function build(root = path.resolve(__dirname, '..'), output = path.join(root, 'public')) {
  fs.rmSync(output, { recursive: true, force: true });
  fs.mkdirSync(output, { recursive: true });
  for (const file of [...PAGES, ...ASSETS]) fs.copyFileSync(path.join(root, 'web', file), path.join(output, file));
  fs.mkdirSync(path.join(output, FONTS));
  for (const file of fs.readdirSync(path.join(root, 'web', FONTS))) {
    if (/\.(woff2|txt)$/.test(file)) fs.copyFileSync(path.join(root, 'web', FONTS, file), path.join(output, FONTS, file));
  }
  const files = fs.readdirSync(output, { recursive: true }).map(f => f.split(path.sep).join('/')).filter(f => f !== FONTS);
  return { output, files: files.sort() };
}

module.exports = { build, PAGES, ASSETS, FONTS };

if (require.main === module) {
  build();
  console.log('Built the TrustGraph site.');
}
