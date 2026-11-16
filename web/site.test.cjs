// DOM-stub logic tests for the landing page; they do not replace browser QA.
// Run from repository root: node web/site.test.cjs
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');

class FakeNode {
  constructor(tag) {
    this.tagName = String(tag).toUpperCase();
    this.attributes = {};
    this.children = [];
    this.listeners = {};
    this.textContent = '';
    this.value = '';
    this.checked = false;
    this.disabled = false;
    this.selectionStart = 0;
    this.selectionEnd = 0;
    this.focused = false;
  }
  append(...items) { for (const item of items) this.children.push(typeof item === 'string' ? { text: item } : item); }
  replaceChildren(...items) { this.children = []; this.textContent = ''; this.append(...items); }
  setAttribute(key, value) { this.attributes[key] = String(value); }
  getAttribute(key) { return this.attributes[key]; }
  addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }
  fire(type, event = {}) { return Promise.all((this.listeners[type] || []).map(fn => fn({ target: this, preventDefault() {}, ...event }))); }
  focus() { this.focused = true; }
  setSelectionRange(a, b) { this.selectionStart = a; this.selectionEnd = b; }
}
const text = node => (node == null ? '' : node.text !== undefined ? node.text : node.textContent + node.children.map(text).join(''));
const all = (node, predicate, out = []) => { if (node && node.children) { if (predicate(node)) out.push(node); node.children.forEach(c => all(c, predicate, out)); } return out; };
const byClass = (node, cls) => all(node, n => (n.attributes && (n.attributes.class || '').split(' ').includes(cls)));

const res = (value, status = 200) => ({ ok: status < 400, status, text: async () => (typeof value === 'string' ? value : JSON.stringify(value)) });

function page() {
  const nodes = new Map();
  const $ = id => { if (!nodes.has(id)) nodes.set(id, new FakeNode('div')); return nodes.get(id); };
  const calls = [];
  const queue = [];
  const context = {
    console, setTimeout, clearTimeout, TRUSTGRAPH_NO_BOOT: true,
    document: { getElementById: $, createElement: tag => new FakeNode(tag), createElementNS: (_, tag) => new FakeNode(tag) },
    fetch: async (url, options) => {
      const body = JSON.parse(options.body);
      calls.push({ url, method: options.method, headers: options.headers, body });
      const next = queue.shift();
      if (!next) throw new Error(`unexpected request ${body.op}`);
      return typeof next === 'function' ? next(body) : next;
    },
  };
  vm.createContext(context);
  vm.runInContext(fs.readFileSync('web/site.js', 'utf8'), context);
  return { $, calls, queue, context, run: code => vm.runInContext(code, context) };
}

const POLICY = 'version: 1\n# starter';
const SUMMARY = {
  agents: ['planner', 'mailer'],
  sources: {
    salaries: { readers: ['hr'], people: ['nina'], agents: ['planner'] },
    roadmap: { readers: ['hr', 'engineering'], people: ['nina', 'ravi', 'sam'], agents: ['planner'] },
    price_sheet: { readers: ['sales', 'customers'], people: ['acme', 'leo'], agents: ['mailer'] },
  },
  actions: {
    email_customer: { agents: ['mailer'], audience: ['customers'], people: ['acme'], approval: { reviewers: ['sales'], people: ['leo'], valid_for_seconds: 1800 } },
    post_to_slack: { agents: ['planner'], audience: ['engineering'], people: ['ravi', 'sam'], approval: null },
  },
  groups: { hr: ['nina'], engineering: ['ravi', 'sam'] },
};
const DENY = { verdict: 'deny', code: 'audience_outside_readers', reason: 'denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included', blocking: [{ source: 'salaries', readers: ['hr'], excluded: ['ravi', 'sam'] }], readers: ['nina'] };
const CHECK_OK = { ok: true, diagnostics: [], tests: [{ name: 'salary data never reaches engineering', expect: 'deny', pass: true, outcome: DENY }], explain: 'salaries: output can reach hr (nina).\npost_to_slack: planner may send to engineering (ravi, sam).', summary: SUMMARY };

async function main() {
  // Request construction for each operation.
  const p = page();
  const req = (op, args) => JSON.parse(p.run(`JSON.stringify(buildRequest(${JSON.stringify(op)}, ${JSON.stringify(args || {})}))`));
  assert.deepEqual(req('examples'), { op: 'examples' });
  assert.deepEqual(req('check', { policy: POLICY }), { op: 'check', policy: POLICY });
  assert.deepEqual(req('decide', { policy: POLICY, output_from: ['salaries', 'roadmap'], agent: 'planner', action: 'post_to_slack' }),
    { op: 'decide', policy: POLICY, query: { output_from: ['salaries', 'roadmap'], agent: 'planner', action: 'post_to_slack', recipients: null } });
  assert.deepEqual(req('decide', { policy: POLICY, output_from: ['roadmap'], agent: 'planner', action: 'post_to_slack', recipients: ['sam'] }).query.recipients, ['sam']);
  assert.deepEqual(req('diff', { old: 'a', new: 'b' }), { op: 'diff', old: 'a', new: 'b' });
  assert.deepEqual(req('simulate', { policy: POLICY, seed: 7 }), { op: 'simulate', policy: POLICY, seed: 7, count: 8, steps: 120 });
  assert.throws(() => p.run('buildRequest("explode")'), /Unknown policy operation/);
  for (const [input, want] of [['0', 0], ['65535', 65535], [' 42 ', 42]]) assert.equal(p.run(`parseSeed(${JSON.stringify(input)})`), want);
  for (const bad of ['', '-1', '65536', '1.5', 'abc', '1e3']) assert.equal(p.run(`parseSeed(${JSON.stringify(bad)})`), null);

  // Coverage labels: every known key has a label; unknown keys fall back to the raw key.
  assert.equal(p.run('coverageLabel("crash_torn_tail")'), 'Crash with a torn journal write');
  assert.equal(p.run('coverageLabel("expiry_boundary_at")'), 'Denied exactly at approval expiry');
  assert.equal(p.run('coverageLabel("brand_new_path")'), 'brand_new_path');
  assert.equal(p.run('coverageLabel("constructor")'), 'constructor');

  // Status line and the interesting default run.
  assert.equal(p.run(`checkStatus(${JSON.stringify(CHECK_OK)}).text`), 'valid · 1 test pass');
  assert.equal(p.run('checkStatus({ok:false,diagnostics:[{severity:"error"},{severity:"error"},{severity:"warning"}]}).text'), '2 errors · 1 warning');
  assert.deepEqual(JSON.parse(p.run(`JSON.stringify(defaultRun(${JSON.stringify(SUMMARY)}))`)), { agent: 'planner', action: 'post_to_slack', sources: ['salaries', 'roadmap'], recipients: null });
  assert.deepEqual(JSON.parse(p.run('JSON.stringify(locatePath("sources:\\n  salaries:\\n    readers: [hrr]\\n", "sources.salaries.readers[0]"))')), [21, 39]);

  // Rendering allow, needs-approval and deny outcomes.
  const decision = (outcome, ctx) => { p.context.__o = outcome; p.context.__c = ctx; const el = new FakeNode('div'); el.replaceChildren(...p.run('renderDecision(__o, __c)')); return el; };
  const deny = decision(DENY, { sources: ['salaries', 'roadmap'], people: ['ravi', 'sam'], approval: null });
  assert.equal(byClass(deny, 'v-deny').length, 1);
  assert.match(text(deny), /DENIED/);
  assert.match(text(deny), /Denied: salaries is readable by hr only/);
  assert.match(text(deny), /Held back by: salaries \(hr only\) — leaves out ravi, sam/);
  assert.match(text(deny), /May read this output: nina/);
  const allow = decision({ verdict: 'allow', code: 'allowed', reason: 'allowed', blocking: [], readers: ['nina', 'ravi', 'sam'] }, { sources: ['roadmap'], people: ['ravi', 'sam'], approval: null });
  assert.match(text(allow), /ALLOWED/);
  assert.doesNotMatch(text(allow), /Held back/);
  assert.equal(byClass(allow, 'v-allow').length, 1);
  assert.match(text(allow), /May read this output: nina, ravi, sam/);
  const wait = decision({ verdict: 'needs_approval', code: 'needs_approval', reason: 'needs approval from sales', blocking: [], readers: ['acme', 'leo'] }, { sources: ['price_sheet'], people: ['acme'], approval: SUMMARY.actions.email_customer.approval });
  assert.match(text(wait), /NEEDS APPROVAL/);
  assert.match(text(wait), /Approval: one of sales \(leo\) approves this exact output, valid 30 minutes\./);

  // Diagnostics with a suggestion; clicking one focuses the editor at its line.
  const pageA = page();
  const policy = pageA.$('policy');
  pageA.queue.push(res('examples down', 500));
  pageA.queue.push(res({ ok: false, diagnostics: [{ severity: 'error', path: 'sources.salaries.readers[0]', code: 'unknown_name', message: '`hrr` is not a group or person in this policy', suggestion: 'did you mean `hr`?' }], tests: null, explain: null, summary: null }));
  policy.value = 'version: 1\nsources:\n  salaries:\n    readers: [hrr]\n';
  await pageA.run('boot()');
  assert.equal(text(pageA.$('check-status')), '1 error');
  const diag = pageA.$('diagnostics');
  assert.match(text(diag), /sources\.salaries\.readers\[0\] — hrr is not a group or person in this policy \(did you mean hr\?\)/);
  await byClass(diag, 'diag')[0].fire('click');
  assert.equal(policy.focused, true);
  assert.equal(policy.value.slice(policy.selectionStart, policy.selectionEnd), '    readers: [hrr]');
  assert.match(text(pageA.$('decision')), /Policy has errors/);
  assert.match(text(pageA.$('tests')), /Fix the errors above/);

  // Full flow: examples → check → decide + diff; then a stale check is ignored.
  const pageB = page();
  pageB.queue.push(res({ examples: [{ id: 'slack-digest', title: 'Slack digest', summary: 's', policy: 'other' }, { id: 'starter', title: 'Starter', summary: 'Starter summary', policy: POLICY }] }));
  pageB.queue.push(res(CHECK_OK));
  pageB.queue.push(res({ ok: true, diagnostics: [], outcome: DENY }));
  await pageB.run('boot()');
  await new Promise(r => setImmediate(r));
  assert.deepEqual(pageB.calls.map(c => c.body.op), ['examples', 'check', 'decide']);
  assert.ok(pageB.calls.every(c => c.url === '/api/policy' && c.method === 'POST' && c.headers['Content-Type'] === 'application/json'));
  assert.equal(pageB.$('example').value, 'starter', 'starts on the starter policy');
  assert.equal(pageB.$('policy').value, POLICY);
  assert.deepEqual(pageB.calls[2].body.query, { output_from: ['salaries', 'roadmap'], agent: 'planner', action: 'post_to_slack', recipients: null });
  assert.match(text(pageB.$('decision')), /DENIED/);
  assert.equal(text(pageB.$('check-status')), 'valid · 1 test pass');
  assert.match(text(pageB.$('diff')), /No changes yet/);
  assert.match(text(pageB.$('explain')), /salaries: output can reach hr/);

  // Unticking a recipient sends an explicit recipient list.
  const samBox = all(pageB.$('recipients'), n => n.tagName === 'INPUT' && n.value === 'sam')[0];
  pageB.queue.push(res({ ok: true, diagnostics: [], outcome: DENY }));
  samBox.checked = false;
  await samBox.fire('change');
  assert.deepEqual(pageB.calls.at(-1).body.query.recipients, ['ravi']);

  // Stale responses: an older, slower check must not overwrite a newer one.
  let releaseOld;
  pageB.queue.push(() => new Promise(r => { releaseOld = () => r(res({ ok: false, diagnostics: [{ severity: 'error', path: '', code: 'yaml_syntax', message: 'old', suggestion: null }] })); }));
  pageB.queue.push(res(CHECK_OK));
  pageB.queue.push(res({ ok: true, diagnostics: [], outcome: DENY }));
  pageB.queue.push(res({ ok: true, diagnostics: [], file: null, widens: true, changes: [{ kind: 'widens', subject: 'source salaries', gained: ['ravi', 'sam'], lost: [], summary: 'readers gained ravi, sam' }] }));
  pageB.$('policy').value = 'edited once';
  const older = pageB.run('runCheck()');
  pageB.$('policy').value = 'edited twice';
  await pageB.run('runCheck()');
  await new Promise(r => setImmediate(r));
  releaseOld();
  await older;
  assert.equal(text(pageB.$('check-status')), 'valid · 1 test pass', 'stale check answer ignored');
  assert.equal(text(pageB.$('diagnostics')), '');
  const diffCall = pageB.calls.find(c => c.body.op === 'diff');
  assert.deepEqual(diffCall.body, { op: 'diff', old: POLICY, new: 'edited twice' });
  assert.match(text(pageB.$('diff')), /WIDENS/);
  assert.match(text(pageB.$('diff')), /A person should approve each WIDENS line/);

  // Non-JSON error body: a calm message, never a parse error.
  pageB.queue.push(res('<html>Bad gateway</html>', 502));
  await pageB.run('runCheck()');
  assert.match(text(pageB.$('check-status')), /^Couldn't check: The server returned 502: <html>Bad gateway/);
  assert.match(text(pageB.$('decision')), /Engine unavailable/);
  pageB.queue.push(res('not json at all', 200));
  await pageB.run('runCheck()');
  assert.match(text(pageB.$('check-status')), /unreadable response/);
  pageB.queue.push(res({ error: 'policy is over 65536 bytes', code: 'policy_too_large' }, 400));
  await pageB.run('runCheck()');
  assert.equal(text(pageB.$('check-status')), "Couldn't check: policy is over 65536 bytes");
  pageB.queue.push(() => Promise.reject(new TypeError('Failed to fetch')));
  await pageB.run('runCheck()');
  assert.match(text(pageB.$('check-status')), /Could not reach the policy engine/);

  // Simulator: invalid seed sends nothing; a run renders counts and coverage; errors re-enable the button.
  const before = pageB.calls.length;
  pageB.$('sim-seed').value = '70000';
  await pageB.run('runSimulation()');
  assert.equal(pageB.calls.length, before);
  assert.match(text(pageB.$('sim-result')), /whole number from 0 to 65535/);
  pageB.$('sim-seed').value = '3';
  pageB.queue.push(res({ ok: true, diagnostics: [], first_seed: 3, count: 4, steps: 80, events: 1234, decisions: { allow: 10, deny: 20, needs_approval: 3 }, violations: [], coverage: { crash_torn_tail: 2, expiry_boundary_at: 0, future_key: 1 }, digest: 'abcdef0123456789abcdef' }));
  await pageB.run('runSimulation()');
  assert.deepEqual(pageB.calls.at(-1).body, { op: 'simulate', policy: 'edited twice', seed: 3, count: 8, steps: 120 });
  const sim = text(pageB.$('sim-result'));
  assert.match(sim, /4 runs, 1,234 events: 10 allow, 20 deny, 3 needs_approval — 0 violations/);
  assert.match(sim, /Crash with a torn journal write/);
  assert.match(sim, /Denied exactly at approval expiry ×0 \(not exercised\)/);
  assert.match(sim, /future_key/);
  assert.match(sim, /Paths exercised: 2 of 3/);
  assert.equal(pageB.$('sim-run').disabled, false);
  pageB.queue.push(res({ ok: false, diagnostics: [], first_seed: 3, count: 4, steps: 80, events: 9, decisions: { allow: 1, deny: 0, needs_approval: 0 }, violations: [{ seed: 4, invariant: 'no_leak', step: 12, detail: 'allow named sam' }], coverage: {}, digest: 'x' }));
  await pageB.run('runSimulation()');
  assert.match(text(pageB.$('sim-result')), /1 safety invariant violation\. This is a bug/);
  assert.match(text(pageB.$('sim-result')), /An allow reached someone outside the readers.*seed 4, step 12/);
  pageB.queue.push(res({ ok: false, diagnostics: [{ severity: 'error', path: 'tests', code: 'test_failed', message: 'test `x` expects deny but the policy says allow', suggestion: null }] }));
  await pageB.run('runSimulation()');
  assert.match(text(pageB.$('sim-result')), /only runs a policy that is valid and passes its own tests.*test x expects deny/);
  pageB.queue.push(res('Service Unavailable', 503));
  await pageB.run('runSimulation()');
  assert.match(text(pageB.$('sim-result')), /The simulator could not run: The server returned 503/);
  assert.equal(pageB.$('sim-run').disabled, false, 'the run button is never left disabled');
  assert.equal(text(pageB.$('sim-run')), 'Run');

  // Examples failing still checks the static textarea content.
  const pageC = page();
  pageC.$('policy').value = POLICY;
  pageC.queue.push(res('gateway', 504));
  pageC.queue.push(res(CHECK_OK));
  pageC.queue.push(res({ ok: true, diagnostics: [], outcome: DENY }));
  await pageC.run('boot()');
  await new Promise(r => setImmediate(r));
  assert.deepEqual(pageC.calls.map(c => c.body.op), ['examples', 'check', 'decide']);
  assert.match(text(pageC.$('decision')), /DENIED/);

  console.log('Landing page logic passed: request shapes, seeds, coverage labels, status line, default run, allow/approval/deny rendering, diagnostics with suggestions, recipients, stale responses, non-JSON and network errors, simulator results and violations.');
}

main().catch(error => { console.error(error); process.exitCode = 1; });
