// Build the static Vercel output in ./public from web/. Every run starts from an
// empty directory, so deleted or renamed pages can never linger in a deployment.
const fs = require('node:fs');
const path = require('node:path');

// Static assets referenced by the published pages. Keep in sync with web/*.html.
const ASSETS = ['story.css', 'story.js', 'framework.css', 'framework.js'];

function replaceOnce(text, anchor, replacement, label) {
  const index = text.indexOf(anchor);
  if (index === -1 || text.indexOf(anchor, index + anchor.length) !== -1) {
    throw new Error(`build_demo: expected exactly one "${anchor}" in ${label}; update scripts/build_demo.cjs.`);
  }
  return text.slice(0, index) + replacement + text.slice(index + anchor.length);
}

function build(root = path.resolve(__dirname, '..'), output = path.join(root, 'public')) {
  fs.rmSync(output, { recursive: true, force: true });
  fs.mkdirSync(output, { recursive: true });
  let html = fs.readFileSync(path.join(root, 'web/story.html'), 'utf8');
  html = replaceOnce(html, '<head>', '<head>\n  <meta name="trustgraph-mode" content="hosted">\n  <meta name="description" content="Explore how TrustGraph enforces source permissions and exact human approval for AI-generated work.">', 'web/story.html');
  html = replaceOnce(html, 'Information boundaries <span', 'Interactive demo · Fictional data · Simulated releases <span', 'web/story.html');
  fs.writeFileSync(path.join(output, 'legacy.html'), html);
  fs.writeFileSync(path.join(output, 'story.html'), html);
  fs.copyFileSync(path.join(root, 'web/framework.html'), path.join(output, 'index.html'));
  for (const file of ASSETS) fs.copyFileSync(path.join(root, 'web', file), path.join(output, file));
  return { output, files: fs.readdirSync(output).sort() };
}

module.exports = { build, replaceOnce, ASSETS };

if (require.main === module) {
  build();
  console.log('Built TrustGraph public demo assets.');
}
