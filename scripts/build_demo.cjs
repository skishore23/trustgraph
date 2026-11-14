const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const output = path.join(root, 'public');
fs.mkdirSync(output, { recursive: true });
const html = fs.readFileSync(path.join(root, 'web/story.html'), 'utf8')
  .replace('<head>', '<head>\n  <meta name="trustgraph-mode" content="hosted">\n  <meta name="description" content="Explore how TrustGraph enforces source permissions and exact human approval for AI-generated work.">')
  .replace('Information boundaries <span', 'Interactive demo · Fictional data · Simulated releases <span');
fs.writeFileSync(path.join(output, 'index.html'), html);
for (const file of ['story.css', 'story.js']) {
  fs.copyFileSync(path.join(root, 'web', file), path.join(output, file));
}
console.log('Built TrustGraph public demo assets.');
