// Verify the Vercel static build is clean, complete and fails loudly on drift.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { build, replaceOnce } = require('./build_demo.cjs');

const root = path.resolve(__dirname, '..');
const output = fs.mkdtempSync(path.join(os.tmpdir(), 'trustgraph-build-'));
try {
  fs.writeFileSync(path.join(output, 'stale.html'), 'old deployment page');
  const { files } = build(root, output);
  assert.ok(!files.includes('stale.html'), 'output directory is emptied before building');
  for (const page of ['index.html', 'story.html', 'legacy.html']) {
    assert.ok(files.includes(page), `${page} is published`);
    const html = fs.readFileSync(path.join(output, page), 'utf8');
    // Every local script/stylesheet a published page references must be published too.
    for (const [, ref] of html.matchAll(/(?:src|href)="\/([^"#?]+\.(?:js|css))"/g)) {
      assert.ok(files.includes(ref), `${page} references missing asset ${ref}`);
    }
    assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/, `${page} has no inline scripts (CSP)`);
    assert.doesNotMatch(html, /\sstyle=|<style/, `${page} has no inline styles (CSP)`);
  }
  const story = fs.readFileSync(path.join(output, 'story.html'), 'utf8');
  assert.match(story, /name="trustgraph-mode" content="hosted"/);
  assert.match(story, /Interactive demo · Fictional data/);
  assert.throws(() => replaceOnce('<body>', '<head>', 'x', 'fixture'), /expected exactly one "<head>"/);
  assert.throws(() => replaceOnce('<head><head>', '<head>', 'x', 'fixture'), /exactly one/);
  console.log('Demo build passed: clean output, anchored rewrites, complete assets, CSP-safe pages.');
} finally {
  fs.rmSync(output, { recursive: true, force: true });
}
