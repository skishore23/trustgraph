// Build the static Vercel output in ./public from web/. Every run starts from an
// empty directory, so deleted or renamed pages can never linger in a deployment.
const fs = require('node:fs');
const path = require('node:path');

// The published site: one page and its two assets. Keep in sync with web/index.html.
const PAGES = ['index.html'];
const ASSETS = ['site.css', 'site.js'];

function build(root = path.resolve(__dirname, '..'), output = path.join(root, 'public')) {
  fs.rmSync(output, { recursive: true, force: true });
  fs.mkdirSync(output, { recursive: true });
  for (const file of [...PAGES, ...ASSETS]) fs.copyFileSync(path.join(root, 'web', file), path.join(output, file));
  return { output, files: fs.readdirSync(output).sort() };
}

module.exports = { build, PAGES, ASSETS };

if (require.main === module) {
  build();
  console.log('Built the TrustGraph site.');
}
