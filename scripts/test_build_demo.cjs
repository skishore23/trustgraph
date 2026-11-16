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
  assert.ok(!files.includes('stale.html'), 'output directory is emptied before building');
  const fonts = files.filter(f => f.startsWith('fonts/'));
  assert.deepEqual(files.filter(f => !f.startsWith('fonts/')), ['index.html', 'site.css', 'site.js'], 'only the playground page, its assets and fonts are published');
  assert.ok(fonts.includes('fonts/LICENSE.txt'), 'the font licence ships with the fonts');
  const html = fs.readFileSync(path.join(output, 'index.html'), 'utf8');
  const css = fs.readFileSync(path.join(output, 'site.css'), 'utf8');
  // Every local script, stylesheet and preloaded font the page references must be published too.
  for (const [, ref] of html.matchAll(/(?:src|href)="\/([^"#?]+\.(?:js|css|woff2))"/g)) {
    assert.ok(files.includes(ref), `index.html references missing asset ${ref}`);
  }
  // Every font the stylesheet references must be published, and every published font used.
  const cssFonts = [...css.matchAll(/url\("?\/([^")]+)"?\)/g)].map(m => m[1]);
  assert.ok(cssFonts.length >= 4, 'site.css declares the self-hosted fonts');
  for (const ref of cssFonts) assert.ok(files.includes(ref), `site.css references missing font ${ref}`);
  for (const font of fonts.filter(f => f.endsWith('.woff2'))) assert.ok(cssFonts.includes(font), `${font} is published but unused`);
  for (const [, ref] of html.matchAll(/rel="preload" href="\/([^"]+)"/g)) assert.ok(cssFonts.includes(ref), `preloaded ${ref} is not declared in site.css`);
  assert.match(html, /<script type="module" src="\/site\.js"><\/script>/);
  assert.match(html, /id="playground"/);
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/, 'no inline scripts (CSP)');
  assert.doesNotMatch(html, /\sstyle=|<style/, 'no inline styles (CSP)');
  assert.doesNotMatch(html, /javascript:|\son[a-z]+=/i, 'no inline handlers (CSP)');
  for (const [, page] of html.matchAll(/href="\/?([^"#?:]+\.html)"/g)) {
    assert.ok(files.includes(page), `index.html links to unpublished page ${page}`);
  }
  console.log('Site build passed: clean output, one page, complete assets and fonts, CSP-safe.');
} finally {
  fs.rmSync(output, { recursive: true, force: true });
}
