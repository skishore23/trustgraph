// Verify the Vercel static build is clean, complete and CSP-safe.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { build } = require('./build_demo.cjs');

const root = path.resolve(__dirname, '..');
const output = fs.mkdtempSync(path.join(os.tmpdir(), 'trustgraph-build-'));
try {
  fs.writeFileSync(path.join(output, 'stale.html'), 'old deployment page');
  const { files } = build(root, output);
  assert.deepEqual(files, ['index.html', 'site.css', 'site.js'], 'only the playground page and its assets are published');
  const html = fs.readFileSync(path.join(output, 'index.html'), 'utf8');
  // Every local script/stylesheet the page references must be published too.
  for (const [, ref] of html.matchAll(/(?:src|href)="\/([^"#?]+\.(?:js|css))"/g)) {
    assert.ok(files.includes(ref), `index.html references missing asset ${ref}`);
  }
  assert.match(html, /<script type="module" src="\/site\.js"><\/script>/);
  assert.match(html, /id="playground"/);
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/, 'no inline scripts (CSP)');
  assert.doesNotMatch(html, /\sstyle=|<style/, 'no inline styles (CSP)');
  assert.doesNotMatch(html, /javascript:|\son[a-z]+=/i, 'no inline handlers (CSP)');
  for (const [, page] of html.matchAll(/href="\/?([^"#?:]+\.html)"/g)) {
    assert.ok(files.includes(page), `index.html links to unpublished page ${page}`);
  }
  console.log('Site build passed: clean output, one page, complete assets, CSP-safe.');
} finally {
  fs.rmSync(output, { recursive: true, force: true });
}
