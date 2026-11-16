'use strict';
// TrustGraph landing page: the policy playground and the simulator.
// The browser loads this file as an ES module. web/site.test.cjs evaluates the
// same file as a plain script with a fake DOM and fetch, so it uses only
// top-level declarations (no import/export) and boots itself unless
// globalThis.TRUSTGRAPH_NO_BOOT is set. The policy text is only ever sent to
// /api/policy.

const API_URL = '/api/policy';
const CHECK_DELAY_MS = 400;
const SIM_COUNT = 8;
const SIM_STEPS = 120;
const SEED_MAX = 65535;
const DEFAULT_EXAMPLE = 'starter';
const SVG_NS = 'http://www.w3.org/2000/svg';

const VERDICTS = {
  allow: { label: 'ALLOWED', word: 'allow', cls: 'allow' },
  needs_approval: { label: 'NEEDS APPROVAL', word: 'needs approval', cls: 'needs_approval' },
  deny: { label: 'DENIED', word: 'deny', cls: 'deny' },
};

// Human labels for the simulator's coverage keys (crates/trustgraph-sim/src/policy_sim).
const COVERAGE_LABELS = {
  approved_allow: 'Approved requests allowed',
  approval_expired_recheck: 'Re-checked after an approval expired',
  expiry_boundary_before: 'Allowed one moment before approval expiry',
  expiry_boundary_at: 'Denied exactly at approval expiry',
  revoke_effective: 'Revoked approvals stop allowing',
  identical_reactivation_with_live_approvals: 'Same policy re-activated while approvals were live',
  recheck_after_identical_reactivation: 'Re-checked after re-activation voided approvals',
  narrowing_with_derived_outputs: 'Source narrowed after outputs were derived from it',
  source_removed_with_derived_outputs: 'Source removed after outputs were derived from it',
  explicit_recipients_check: 'Checks that name explicit recipients',
  agent_denied_source: 'Agent refused a source it may not use',
  crash_clean: 'Crash and replay after a durable write',
  crash_lost_write: 'Crash, then a failed write after restart',
  crash_torn_tail: 'Crash with a torn journal write',
  torn_approve: 'Torn write of an approval',
  torn_revoke: 'Torn write of a revocation',
  torn_activate: 'Torn write of a policy activation',
  crash_without_resend: 'Crash with nothing re-sent first',
};

const INVARIANT_LABELS = {
  no_leak: 'An allow reached someone outside the readers of every input',
  approval_scoped: 'An approval outlived a policy change or covered another request',
  replay_equivalent: 'Crash and replay changed the state or the next decisions',
  narrowing_applies: 'Narrowing a source did not apply to what was derived from it',
  agents_cannot_change_policy: 'An agent call changed the active policy',
};

/* ---------- pure helpers ---------- */

const plural = (n, word, many = `${word}s`) => `${n} ${n === 1 ? word : many}`;
const list = value => (Array.isArray(value) ? value.map(String) : []);
const hasOwn = (object, key) => object != null && Object.prototype.hasOwnProperty.call(object, key);

function coverageLabel(key) {
  return hasOwn(COVERAGE_LABELS, key) ? COVERAGE_LABELS[key] : String(key);
}

function invariantLabel(key) {
  return hasOwn(INVARIANT_LABELS, key) ? INVARIANT_LABELS[key] : String(key);
}

// Build the JSON body for one /api/policy operation.
function buildRequest(op, args = {}) {
  switch (op) {
    case 'examples':
      return { op };
    case 'check':
      return { op, policy: String(args.policy ?? '') };
    case 'decide':
      return {
        op,
        policy: String(args.policy ?? ''),
        query: {
          output_from: list(args.output_from),
          agent: String(args.agent ?? ''),
          action: String(args.action ?? ''),
          recipients: Array.isArray(args.recipients) ? args.recipients.map(String) : null,
        },
      };
    case 'diff':
      return { op, old: String(args.old ?? ''), new: String(args.new ?? '') };
    case 'simulate':
      return { op, policy: String(args.policy ?? ''), seed: args.seed, count: args.count ?? SIM_COUNT, steps: args.steps ?? SIM_STEPS };
    default:
      throw new Error(`Unknown policy operation: ${op}`);
  }
}

// A seed is a whole number from 0 to 65535, or null.
function parseSeed(value) {
  const text = String(value ?? '').trim();
  if (!/^\d{1,5}$/.test(text)) return null;
  const n = Number(text);
  return n <= SEED_MAX ? n : null;
}

function humanDuration(seconds) {
  const s = Number(seconds);
  if (!Number.isFinite(s) || s <= 0) return '';
  if (s % 86400 === 0) return plural(s / 86400, 'day');
  if (s % 3600 === 0) return plural(s / 3600, 'hour');
  if (s % 60 === 0) return plural(s / 60, 'minute');
  return plural(s, 'second');
}

function countDiagnostics(diagnostics) {
  const all = Array.isArray(diagnostics) ? diagnostics : [];
  const errors = all.filter(d => d && d.severity === 'error').length;
  return { errors, warnings: all.length - errors };
}

// The one-line editor status: text plus a class for colour (never colour alone).
function checkStatus(result) {
  const { errors, warnings } = countDiagnostics(result && result.diagnostics);
  const warn = warnings ? ` · ${plural(warnings, 'warning')}` : '';
  if (errors) return { cls: 'bad', text: `✗ ${plural(errors, 'error')}${warn}` };
  const tests = Array.isArray(result && result.tests) ? result.tests : [];
  const failed = tests.filter(t => !t.pass).length;
  if (failed) return { cls: 'bad', text: `✗ valid · ${failed} of ${plural(tests.length, 'test')} fail${warn}` };
  if (!tests.length) return { cls: 'warn', text: `✓ valid · no tests${warn}` };
  return { cls: warnings ? 'warn' : 'ok', text: `✓ valid · ${plural(tests.length, 'test')} pass${warn}` };
}

// Pick a first run that shows something interesting: an agent that may use one
// source the action's audience cannot read and one it can, sent to that action.
function defaultRun(summary) {
  const sources = Object.entries((summary && summary.sources) || {});
  const actions = Object.entries((summary && summary.actions) || {});
  const covers = (source, action) => list(action.people).every(p => list(source.people).includes(p));
  for (const wantApproval of [false, true]) {
    for (const [name, action] of actions) {
      if (Boolean(action.approval) !== wantApproval) continue;
      for (const agent of list(action.agents)) {
        const usable = sources.filter(([, s]) => list(s.agents).includes(agent));
        const sensitive = usable.find(([, s]) => !covers(s, action));
        const harmless = usable.find(([, s]) => covers(s, action));
        if (sensitive && harmless) return { agent, action: name, sources: [sensitive[0], harmless[0]], recipients: null };
      }
    }
  }
  const [name, action] = actions[0] || [];
  const agent = (action && list(action.agents)[0]) || list(summary && summary.agents)[0] || '';
  const usable = sources.filter(([, s]) => list(s.agents).includes(agent)).map(([n]) => n);
  return { agent, action: name || '', sources: usable.slice(0, 1), recipients: null };
}

// Keep what the visitor chose when the policy changes, dropping names that vanished.
function reconcileRun(run, summary) {
  if (!run) return defaultRun(summary);
  const agents = list(summary.agents);
  const sources = Object.keys(summary.sources || {});
  const actions = summary.actions || {};
  const fallback = defaultRun(summary);
  const action = hasOwn(actions, run.action) ? run.action : fallback.action;
  const people = list(actions[action] && actions[action].people);
  let recipients = null;
  if (action === run.action && Array.isArray(run.recipients)) {
    recipients = run.recipients.filter(p => people.includes(p));
    if (recipients.length === people.length) recipients = null;
  }
  return {
    agent: agents.includes(run.agent) ? run.agent : fallback.agent,
    action,
    sources: run.sources.filter(s => sources.includes(s)),
    recipients,
  };
}

function expandRecipients(run, summary) {
  const action = summary && summary.actions && summary.actions[run.action];
  if (!action) return [];
  return Array.isArray(run.recipients) ? run.recipients.slice() : list(action.people);
}

// Find the line a diagnostic path points at: "sources.comp_review.readers[1]".
function locatePath(text, path) {
  const keys = String(path || '').split('.').map(k => k.replace(/\[\d+\]/g, '')).filter(Boolean);
  let from = 0;
  let found = -1;
  for (const key of keys) {
    const escaped = key.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const re = new RegExp(`^[ \\t]*(?:-[ \\t]*)?${escaped}[ \\t]*:`, 'm');
    const match = re.exec(text.slice(from));
    if (!match) break;
    found = from + match.index;
    from = found + match[0].length;
  }
  if (found < 0) return null;
  const end = text.indexOf('\n', found);
  return [found, end === -1 ? text.length : end];
}

// Read the body as text first: proxies and extractor rejections may not send JSON.
async function readJson(response) {
  let text = '';
  try { text = await response.text(); } catch { /* treat an unreadable body as empty */ }
  let value = null;
  try { value = text ? JSON.parse(text) : null; } catch { value = null; }
  if (!response.ok) {
    if (value && typeof value.error === 'string') throw new Error(value.error);
    throw new Error(`The server returned ${response.status}${text ? `: ${text.slice(0, 160)}` : ''}`);
  }
  if (!value || typeof value !== 'object') throw new Error('The server returned an unreadable response.');
  return value;
}

async function post(body, signal) {
  let response;
  try {
    response = await fetch(API_URL, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal });
  } catch (error) {
    if (error && error.name === 'AbortError') throw error;
    throw new Error('Could not reach the policy engine. Check your connection and try again.');
  }
  return readJson(response);
}

// One request lane per operation: a newer request makes older answers stale,
// and a stale answer is dropped rather than rendered.
function lane() {
  let seq = 0;
  let controller = null;
  return {
    async run(body) {
      seq += 1;
      const mine = seq;
      if (controller) controller.abort();
      controller = typeof AbortController === 'function' ? new AbortController() : null;
      try {
        const value = await post(body, controller ? controller.signal : undefined);
        return mine === seq ? { fresh: true, value } : { fresh: false };
      } catch (error) {
        if (mine !== seq || (error && error.name === 'AbortError')) return { fresh: false };
        return { fresh: true, error };
      }
    },
    cancel() {
      seq += 1;
      if (controller) controller.abort();
      controller = null;
    },
  };
}

/* ---------- DOM building ---------- */

function h(tag, props, ...children) {
  const el = document.createElement(tag);
  applyProps(el, props);
  appendAll(el, children);
  return el;
}

function svg(tag, attrs, ...children) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs || {})) if (value != null) el.setAttribute(key, String(value));
  appendAll(el, children);
  return el;
}

function applyProps(el, props) {
  if (!props) return;
  for (const [key, value] of Object.entries(props)) {
    if (value == null || value === false) continue;
    if (key === 'class') el.setAttribute('class', value);
    else if (key === 'text') el.textContent = String(value);
    else if (key === 'on') for (const [type, fn] of Object.entries(value)) el.addEventListener(type, fn);
    else if (key === 'checked' || key === 'disabled' || key === 'value' || key === 'type' || key === 'name') el[key] = value;
    else el.setAttribute(key, String(value));
  }
}

function appendAll(el, children) {
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false) continue;
    el.append(typeof child === 'object' ? child : String(child));
  }
}

// Turn `backticked` names in engine text into styled spans.
function richText(text) {
  return String(text ?? '').split('`').map((part, i) => (i % 2 ? h('span', { class: 'name', text: part }) : part)).filter(p => p !== '');
}

function peopleChips(people, cls) {
  if (!people.length) return h('span', { class: 'muted', text: 'no one' });
  return h('span', { class: 'people-list' }, people.map(p => h('span', { class: cls ? `person ${cls}` : 'person', text: p })));
}

function verdictIcon(verdict) {
  const shapes = {
    allow: svg('path', { d: 'M10 17.5 15 22.5 24.5 12' }),
    needs_approval: svg('path', { d: 'M17 10v7.5l4.5 3' }),
    deny: svg('path', { d: 'M12 12 22 22M22 12 12 22' }),
  };
  return svg('svg', { class: 'verdict-icon', viewBox: '0 0 34 34', 'aria-hidden': 'true' }, svg('circle', { cx: 17, cy: 17, r: 17 }), shapes[verdict]);
}

function renderDiagnostics(diagnostics, onPick) {
  return (Array.isArray(diagnostics) ? diagnostics : []).map(d => {
    const severity = d.severity === 'warning' ? 'warning' : 'error';
    const button = h('button', { class: `diag ${severity}`, type: 'button', on: { click: () => onPick && onPick(d) } },
      h('span', { class: 'diag-head' }, h('span', { class: 'diag-sev', text: severity }), d.path ? h('span', { class: 'diag-path', text: d.path }) : null),
      h('span', { class: 'diag-msg' }, richText(d.message)),
      d.suggestion ? h('span', { class: 'diag-suggest' }, richText(capitalise(d.suggestion))) : null);
    return h('li', null, button);
  });
}

const capitalise = text => String(text).charAt(0).toUpperCase() + String(text).slice(1);

function renderTests(tests) {
  if (!Array.isArray(tests)) return [h('li', { class: 'muted', text: 'Fix the errors above to run the tests.' })];
  if (!tests.length) return [h('li', { class: 'muted', text: 'This policy has no tests. Add some under tests:.' })];
  return tests.map(t => {
    const pass = Boolean(t.pass);
    const got = t.outcome && VERDICTS[t.outcome.verdict];
    const expected = VERDICTS[t.expect];
    const why = pass ? null : h('span', { class: 'test-why' },
      `Expected ${expected ? expected.word : t.expect}, got ${got ? got.word : 'no decision'}`,
      t.outcome && t.outcome.reason ? [' — ', richText(t.outcome.reason)] : null);
    return h('li', { class: `test ${pass ? 'pass' : 'fail'}` }, h('span', { class: 'test-tag', text: pass ? 'PASS' : 'FAIL' }), h('span', { text: t.name }), why);
  });
}

function renderExplain(text) {
  const lines = String(text || '').split('\n').map(l => l.trim()).filter(Boolean);
  if (!lines.length) return [h('li', { class: 'muted', text: 'Appears once the policy is valid.' })];
  return lines.map(line => {
    const colon = line.indexOf(':');
    if (colon <= 0) return h('li', null, richText(line));
    return h('li', null, h('span', { class: 'name', text: line.slice(0, colon) }), richText(line.slice(colon)));
  });
}

const DIFF_TAGS = { widens: 'WIDENS', narrows: 'NARROWS', neutral: 'CHANGE' };

function renderDiff(result) {
  if (!result) return [h('li', { class: 'empty-state' }, 'No changes yet — try adding ', h('code', { text: 'engineering' }), " to the salary source's readers.")];
  if (!result.ok && result.file) {
    return [h('li', { class: 'empty-state', text: result.file === 'old' ? 'The original policy could not be read.' : 'Fix the errors in the policy to compare it with the original.' })];
  }
  const changes = Array.isArray(result.changes) ? result.changes : [];
  if (!changes.length) return [h('li', { class: 'empty-state', text: 'No change in who can read or receive anything.' })];
  const items = changes.map(c => {
    const kind = hasOwn(DIFF_TAGS, c.kind) ? c.kind : 'neutral';
    return h('li', { class: kind }, h('span', { class: 'diff-tag', text: DIFF_TAGS[kind] }), h('span', null, h('span', { class: 'name', text: c.subject }), `: ${c.summary}`));
  });
  if (result.widens) items.unshift(h('li', null, h('span', { class: 'diff-alert', text: 'This version gives more people access. A human should approve every WIDENS line.' })));
  return items;
}

// Sources → output → recipients. Blocking sources and refused recipients are marked.
function lineageDiagram(outcome, context) {
  const sources = context.sources.length ? context.sources : ['(nothing)'];
  const people = context.people;
  const readers = new Set(list(outcome.readers));
  const blocking = new Set((outcome.blocking || []).map(b => b.source));
  const verdict = outcome.verdict;
  const rows = Math.max(sources.length, people.length, 1);
  const rowH = 34;
  const top = 26;
  const height = top + rows * rowH + 4;
  const width = 360;
  const mid = top + (rows * rowH) / 2 - 2;
  const short = (text, n) => (text.length > n ? `${text.slice(0, n - 1)}…` : text);
  const y = (i, n) => top + ((rows - n) * rowH) / 2 + i * rowH;
  const nodes = [];
  const edges = [];
  sources.forEach((name, i) => {
    const ny = y(i, sources.length);
    const cls = blocking.has(name) ? 'l-node blocking' : 'l-node';
    nodes.push(svg('g', { class: cls }, svg('title', null, name), svg('rect', { x: 0, y: ny, width: 112, height: 26, rx: 6 }), svg('text', { x: 9, y: ny + 17 }, short(name, 13))));
    edges.push(svg('path', { class: blocking.has(name) ? 'l-edge blocking' : 'l-edge', d: `M112 ${ny + 13} C136 ${ny + 13} 132 ${mid + 13} 150 ${mid + 13}` }));
  });
  nodes.push(svg('g', { class: 'l-node l-out' }, svg('rect', { x: 150, y: mid, width: 66, height: 26, rx: 13 }), svg('text', { x: 162, y: mid + 17 }, 'output')));
  const edgeCls = verdict === 'allow' ? 'l-edge ok' : verdict === 'needs_approval' ? 'l-edge wait' : 'l-edge blocked';
  people.forEach((person, i) => {
    const ny = y(i, people.length);
    const inReaders = readers.has(person);
    const nodeCls = !inReaders ? 'l-node left-out' : verdict === 'allow' ? 'l-node ok' : verdict === 'needs_approval' ? 'l-node wait' : 'l-node';
    edges.push(svg('path', { class: edgeCls, d: `M216 ${mid + 13} C236 ${mid + 13} 232 ${ny + 13} 256 ${ny + 13}` }));
    if (verdict === 'deny') edges.push(svg('g', { class: 'l-x', transform: `translate(246 ${ny + 13})` }, svg('circle', { r: 6.5 }), svg('path', { d: 'M-2.6-2.6 2.6 2.6M2.6-2.6-2.6 2.6' })));
    nodes.push(svg('g', { class: nodeCls }, svg('title', null, person), svg('rect', { x: 256, y: ny, width: 104, height: 26, rx: 6 }), svg('text', { x: 265, y: ny + 17 }, short(person, 12))));
  });
  const label = `${sources.join(', ')} flow into the output; ${verdict === 'deny' ? 'blocked from' : verdict === 'needs_approval' ? 'waiting for approval to reach' : 'sent to'} ${people.join(', ') || 'no one'}.`;
  return svg('svg', { class: 'lineage', viewBox: `0 0 ${width} ${height}`, role: 'img', 'aria-label': label },
    svg('text', { class: 'l-caption', x: 0, y: 12 }, 'READ'),
    svg('text', { class: 'l-caption', x: 150, y: 12 }, 'OUTPUT'),
    svg('text', { class: 'l-caption', x: 256, y: 12 }, 'SENT TO'),
    edges, nodes);
}

// The whole decision panel for one outcome.
function renderDecision(outcome, context) {
  const info = VERDICTS[outcome && outcome.verdict];
  if (!info) return [h('div', { class: 'verdict verdict-error' }, h('span', { class: 'verdict-text', text: 'No decision' })), h('p', { class: 'muted', text: 'The engine did not return a verdict.' })];
  const blocking = Array.isArray(outcome.blocking) ? outcome.blocking : [];
  const facts = [];
  if (blocking.length) {
    facts.push(h('div', null, h('dt', { text: 'Held back by' }), h('dd', null, blocking.map(b => h('span', { class: 'blocker' },
      h('span', { class: 'name', text: b.source }), ` is for ${list(b.readers).join(', ') || 'no one'} — leaves out `, peopleChips(list(b.excluded), 'out'))))));
  }
  facts.push(h('div', null, h('dt', { text: 'Who may read this output' }), h('dd', null, peopleChips(list(outcome.readers), 'in'))));
  const approval = context.approval;
  if (outcome.verdict === 'needs_approval' && approval) {
    const lasts = humanDuration(approval.valid_for_seconds);
    facts.push(h('div', null, h('dt', { text: 'Approval' }), h('dd', null,
      `A reviewer from ${list(approval.reviewers).join(', ')} (${list(approval.people).join(', ')}) must approve this exact output`,
      lasts ? `. The approval lasts ${lasts}.` : '.')));
  }
  return [
    h('div', { class: `verdict verdict-${info.cls}` }, verdictIcon(outcome.verdict), h('span', { class: 'verdict-text', text: info.label })),
    h('p', { class: 'reason' }, richText(capitalise(outcome.reason || ''))),
    h('dl', null, facts),
    lineageDiagram(outcome, context),
  ];
}

function renderSimulation(result) {
  if (!result.ok && Array.isArray(result.diagnostics) && countDiagnostics(result.diagnostics).errors) {
    const errors = result.diagnostics.filter(d => d && d.severity === 'error');
    return [
      h('p', { class: 'inline-error', text: 'The simulator only runs a policy that is valid and passes its own tests. Fix this first:' }),
      h('ul', { class: 'sim-errors' }, errors.map(d => h('li', null, richText(d.message)))),
    ];
  }
  const d = result.decisions || {};
  const violations = Array.isArray(result.violations) ? result.violations : [];
  const stat = (cls, label, value) => h('div', { class: `stat ${cls}` }, h('dt', { text: label }), h('dd', { text: Number(value || 0).toLocaleString('en-US') }));
  const out = [h('dl', { class: 'stats' },
    stat('', 'Runs', result.count),
    stat('', 'Events', result.events),
    stat('allow', 'Allowed', d.allow),
    stat('needs_approval', 'Needs approval', d.needs_approval),
    stat('deny', 'Denied', d.deny),
    stat(`violations ${violations.length ? 'some' : 'none'}`, 'Violations', violations.length))];
  if (violations.length) {
    out.push(h('div', { class: 'violations-list', role: 'alert' },
      h('h3', { text: `${plural(violations.length, 'safety invariant violation')} found` }),
      h('ul', null, violations.map(v => h('li', null,
        h('b', { text: invariantLabel(v.invariant) }), ` (seed ${v.seed}, step ${v.step}): ${v.detail}`))),
      h('p', { class: 'hint', text: 'This is a bug in TrustGraph, not in your policy. Please report it with the seed.' })));
  }
  const coverage = result.coverage && typeof result.coverage === 'object' ? result.coverage : {};
  const known = Object.keys(COVERAGE_LABELS).filter(k => hasOwn(coverage, k));
  const extra = Object.keys(coverage).filter(k => !hasOwn(COVERAGE_LABELS, k)).sort();
  const keys = [...known, ...extra];
  if (keys.length) {
    const hit = keys.filter(k => Number(coverage[k]) > 0).length;
    out.push(h('div', { class: 'coverage' },
      h('h3', { text: `Paths exercised: ${hit} of ${keys.length}` }),
      h('p', { class: 'hint', text: '○ means this seed did not reach that path. Try another seed.' }),
      h('ul', null, keys.map(k => {
        const n = Number(coverage[k]) || 0;
        return h('li', { class: `cov ${n ? 'hit' : 'miss'}` },
          h('span', { class: 'cov-mark', 'aria-hidden': 'true', text: n ? '✓' : '○' }),
          h('span', null, coverageLabel(k), h('span', { class: 'sr-only', text: n ? ' (exercised)' : ' (not exercised)' })),
          h('span', { class: 'cov-n', text: `×${n}` }));
      }))));
  }
  if (result.digest) out.push(h('p', { class: 'digest' }, `First seed ${result.first_seed ?? ''} · ${result.steps ?? ''} steps per run · digest ${String(result.digest).slice(0, 16)}… The same seed gives the same digest.`));
  return out;
}

/* ---------- the live page ---------- */

const ui = {};
const state = {
  examples: [],
  exampleId: null,
  original: null,
  summary: null,
  run: null,
  valid: false,
  escaped: false,
  timer: null,
};
const lanes = { examples: lane(), check: lane(), decide: lane(), diff: lane(), simulate: lane() };

function $(id) { return document.getElementById(id); }

function setStatus(cls, text) {
  ui.status.setAttribute('class', `check-status ${cls}`);
  ui.status.textContent = text;
}

function showDecisionMessage(title, detail, cls = 'pending') {
  ui.decision.replaceChildren(
    h('div', { class: `verdict verdict-${cls}` }, h('span', { class: 'verdict-text', text: title })),
    detail ? h('p', { class: 'muted', text: detail }) : null);
}

function fillSelect(select, values, selected) {
  select.replaceChildren(...values.map(v => h('option', { value: v, text: v })));
  select.value = selected;
  select.disabled = values.length === 0;
}

async function loadExamples() {
  setStatus('pending', 'Loading examples…');
  const res = await lanes.examples.run(buildRequest('examples'));
  if (!res.fresh) return;
  const examples = res.value && Array.isArray(res.value.examples) ? res.value.examples : [];
  if (res.error || !examples.length) {
    state.original = ui.policy.value;
    setStatus('offline', `Examples unavailable: ${res.error ? res.error.message : 'none returned'}`);
    await runCheck();
    return;
  }
  state.examples = examples;
  const start = examples.some(e => e.id === DEFAULT_EXAMPLE) ? DEFAULT_EXAMPLE : examples[0].id;
  ui.example.replaceChildren(...examples.map(e => h('option', { value: e.id, text: e.title || e.id })));
  ui.example.disabled = false;
  await loadExample(start);
}

async function loadExample(id) {
  const example = state.examples.find(e => e.id === id);
  if (!example) return;
  state.exampleId = id;
  ui.example.value = id;
  ui.summaryLine.textContent = example.summary || '';
  ui.policy.value = example.policy;
  state.original = example.policy;
  state.run = null;
  lanes.simulate.cancel();
  ui.simResult.replaceChildren(h('p', { class: 'muted', text: 'Runs use the policy currently in the playground editor.' }));
  ui.simRun.disabled = false;
  await runCheck();
}

function scheduleCheck() {
  clearTimeout(state.timer);
  setStatus('pending', 'Checking…');
  state.timer = setTimeout(() => { runCheck(); }, CHECK_DELAY_MS);
}

async function runCheck() {
  clearTimeout(state.timer);
  const policy = ui.policy.value;
  const res = await lanes.check.run(buildRequest('check', { policy }));
  if (!res.fresh) return;
  if (res.error) {
    state.valid = false;
    setStatus('offline', `Couldn't check: ${res.error.message}`);
    lanes.decide.cancel();
    showDecisionMessage('Engine unavailable', 'The decision will appear when the policy engine answers. Edit the policy to try again.', 'error');
    return;
  }
  const result = res.value;
  const status = checkStatus(result);
  setStatus(status.cls, status.text);
  ui.diagnostics.replaceChildren(...renderDiagnostics(result.diagnostics, focusDiagnostic));
  ui.tests.replaceChildren(...renderTests(result.tests));
  ui.explain.replaceChildren(...renderExplain(result.explain));
  state.valid = Boolean(result.ok && result.summary);
  if (state.valid) {
    state.summary = result.summary;
    state.run = reconcileRun(state.run, state.summary);
    renderRunControls();
    runDecide();
  } else {
    lanes.decide.cancel();
    showDecisionMessage('Policy has errors', 'Fix the errors in the policy to see a decision.', 'error');
  }
  runDiff(policy, Boolean(result.ok));
}

async function runDecide() {
  if (!state.valid || !state.run) return;
  const run = state.run;
  if (Array.isArray(run.recipients) && !run.recipients.length) {
    lanes.decide.cancel();
    showDecisionMessage('No recipients', 'Tick at least one recipient.');
    return;
  }
  ui.decision.setAttribute('aria-busy', 'true');
  const res = await lanes.decide.run(buildRequest('decide', { policy: ui.policy.value, output_from: run.sources, agent: run.agent, action: run.action, recipients: run.recipients }));
  if (!res.fresh) return;
  ui.decision.setAttribute('aria-busy', 'false');
  if (res.error) { showDecisionMessage('Engine unavailable', res.error.message, 'error'); return; }
  if (!res.value.outcome) { showDecisionMessage('Policy has errors', 'Fix the errors in the policy to see a decision.', 'error'); return; }
  const action = state.summary.actions[run.action] || {};
  ui.decision.replaceChildren(...renderDecision(res.value.outcome, {
    sources: run.sources,
    people: expandRecipients(run, state.summary),
    approval: action.approval || null,
  }));
}

async function runDiff(policy, ok) {
  if (state.original == null || policy === state.original) {
    lanes.diff.cancel();
    ui.diff.replaceChildren(...renderDiff(null));
    return;
  }
  if (!ok) {
    lanes.diff.cancel();
    ui.diff.replaceChildren(...renderDiff({ ok: false, file: 'new' }));
    return;
  }
  const res = await lanes.diff.run(buildRequest('diff', { old: state.original, new: policy }));
  if (!res.fresh) return;
  if (res.error) { ui.diff.replaceChildren(h('li', { class: 'empty-state', text: `Couldn't compare: ${res.error.message}` })); return; }
  ui.diff.replaceChildren(...renderDiff(res.value));
}

function renderRunControls() {
  const summary = state.summary;
  const run = state.run;
  fillSelect(ui.agent, list(summary.agents), run.agent);
  fillSelect(ui.action, Object.keys(summary.actions || {}), run.action);

  const sources = Object.entries(summary.sources || {});
  ui.sources.replaceChildren(...sources.map(([name, s]) => {
    const usable = list(s.agents).includes(run.agent);
    return h('label', { class: 'check' },
      h('input', { type: 'checkbox', value: name, checked: run.sources.includes(name), on: { change: event => toggleSource(name, event.target.checked) } }),
      h('span', { class: 'check-name', text: name }),
      h('span', { class: 'check-meta', text: usable ? `for ${list(s.readers).join(', ')}` : `${run.agent} may not use` }));
  }));

  const action = summary.actions[run.action] || {};
  const approval = action.approval;
  ui.actionHelp.textContent = `Audience: ${list(action.audience).join(', ')} (${list(action.people).join(', ')})${approval ? ` · approval by ${list(approval.reviewers).join(', ')}` : ''}`;
  const chosen = expandRecipients(run, summary);
  ui.recipients.replaceChildren(...list(action.people).map(person => h('label', { class: 'check' },
    h('input', { type: 'checkbox', value: person, checked: chosen.includes(person), on: { change: event => toggleRecipient(person, event.target.checked) } }),
    h('span', { class: 'check-name', text: person }))));
}

function toggleSource(name, on) {
  const set = new Set(state.run.sources);
  if (on) set.add(name); else set.delete(name);
  const order = Object.keys(state.summary.sources || {});
  state.run.sources = order.filter(s => set.has(s));
  runDecide();
}

function toggleRecipient(person, on) {
  const people = list(state.summary.actions[state.run.action].people);
  const set = new Set(expandRecipients(state.run, state.summary));
  if (on) set.add(person); else set.delete(person);
  const chosen = people.filter(p => set.has(p));
  state.run.recipients = chosen.length === people.length ? null : chosen;
  runDecide();
}

function focusDiagnostic(diagnostic) {
  ui.policy.focus();
  const range = locatePath(ui.policy.value, diagnostic.path);
  if (range && typeof ui.policy.setSelectionRange === 'function') ui.policy.setSelectionRange(range[0], range[1]);
}

function insertIndent(textarea) {
  let done = false;
  try { done = typeof document.execCommand === 'function' && document.execCommand('insertText', false, '  '); } catch { done = false; }
  if (done) return;
  const { selectionStart: a, selectionEnd: b, value } = textarea;
  textarea.value = `${value.slice(0, a)}  ${value.slice(b)}`;
  textarea.selectionStart = textarea.selectionEnd = a + 2;
  scheduleCheck();
}

function onEditorKey(event) {
  if (event.key === 'Escape') { state.escaped = true; return; }
  if (event.key === 'Tab' && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey && !state.escaped) {
    event.preventDefault();
    insertIndent(ui.policy);
  }
  state.escaped = false;
}

async function runSimulation(event) {
  if (event) event.preventDefault();
  const seed = parseSeed(ui.seed.value);
  if (seed === null) {
    ui.simResult.replaceChildren(h('p', { class: 'inline-error', text: 'Enter a whole number from 0 to 65535 as the seed.' }));
    return;
  }
  ui.simRun.disabled = true;
  ui.simRun.textContent = 'Running…';
  ui.simResult.setAttribute('aria-busy', 'true');
  try {
    const res = await lanes.simulate.run(buildRequest('simulate', { policy: ui.policy.value, seed }));
    if (!res.fresh) return;
    if (res.error) ui.simResult.replaceChildren(h('p', { class: 'inline-error', text: `The simulator could not run: ${res.error.message}` }));
    else ui.simResult.replaceChildren(...renderSimulation(res.value));
  } finally {
    ui.simRun.disabled = false;
    ui.simRun.textContent = 'Run simulator';
    ui.simResult.setAttribute('aria-busy', 'false');
  }
}

function boot() {
  Object.assign(ui, {
    example: $('example'), summaryLine: $('example-summary'), policy: $('policy'), status: $('check-status'),
    reset: $('reset'), diagnostics: $('diagnostics'), tests: $('tests'), agent: $('agent'), sources: $('sources'),
    action: $('action'), actionHelp: $('action-help'), recipients: $('recipients'), decision: $('decision'),
    explain: $('explain'), diff: $('diff'), simForm: $('sim-form'), seed: $('sim-seed'), simRun: $('sim-run'), simResult: $('sim-result'),
  });
  if (!ui.policy) return;
  ui.policy.addEventListener('input', scheduleCheck);
  ui.policy.addEventListener('keydown', onEditorKey);
  ui.example.addEventListener('change', () => loadExample(ui.example.value));
  ui.reset.addEventListener('click', () => {
    if (state.original == null) return;
    ui.policy.value = state.original;
    state.run = null;
    runCheck();
  });
  ui.agent.addEventListener('change', () => { state.run.agent = ui.agent.value; renderRunControls(); runDecide(); });
  ui.action.addEventListener('change', () => { state.run.action = ui.action.value; state.run.recipients = null; renderRunControls(); runDecide(); });
  ui.simForm.addEventListener('submit', runSimulation);
  return loadExamples();
}

if (!globalThis.TRUSTGRAPH_NO_BOOT) boot();
