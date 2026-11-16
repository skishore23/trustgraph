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
const DEFAULT_EXAMPLE = 'handoff';
const STARTER = 'starter';
const MAX_UI_STEPS = 3;

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
    case 'run':
      return {
        op,
        policy: String(args.policy ?? ''),
        steps: (Array.isArray(args.steps) ? args.steps : []).map(s => ({ agent: String(s.agent ?? ''), reads: list(s.reads), writes: String(s.writes ?? '') })),
        check: args.check
          ? { step: Number(args.check.step), action: String(args.check.action ?? ''), recipients: Array.isArray(args.check.recipients) ? args.check.recipients.map(String) : null }
          : null,
      };
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
  if (errors) return { cls: 'bad', text: `${plural(errors, 'error')}${warn}` };
  const tests = Array.isArray(result && result.tests) ? result.tests : [];
  const failed = tests.filter(t => !t.pass).length;
  if (failed) return { cls: 'bad', text: `valid · ${failed} of ${plural(tests.length, 'test')} fail${warn}` };
  if (!tests.length) return { cls: 'warn', text: `valid · no tests${warn}` };
  return { cls: warnings ? 'warn' : 'ok', text: `valid · ${plural(tests.length, 'test')} pass${warn}` };
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

// A run is a list of steps, each an agent that read some sources and some
// earlier steps' outputs; the last step's output is checked against an action.
// { steps: [{ agent, sources: [names], prior: [step indexes] }], action, recipients }
function toRun(simple) {
  return { steps: [{ agent: simple.agent, sources: list(simple.sources), prior: [] }], action: simple.action, recipients: simple.recipients ?? null };
}

const cloneRun = run => JSON.parse(JSON.stringify(run));

// Keep what the visitor chose when the policy changes, dropping names that vanished.
function reconcileRun(run, summary) {
  const fallback = toRun(defaultRun(summary));
  if (!run || !Array.isArray(run.steps) || !run.steps.length) return fallback;
  const agents = list(summary.agents);
  const sources = Object.keys(summary.sources || {});
  const actions = summary.actions || {};
  const action = hasOwn(actions, run.action) ? run.action : fallback.action;
  const people = list(actions[action] && actions[action].people);
  let recipients = null;
  if (action === run.action && Array.isArray(run.recipients)) {
    recipients = run.recipients.filter(p => people.includes(p));
    if (recipients.length === people.length) recipients = null;
  }
  const steps = run.steps.map((step, i) => ({
    agent: agents.includes(step.agent) ? step.agent : fallback.steps[0].agent,
    sources: list(step.sources).filter(x => sources.includes(x)),
    prior: (Array.isArray(step.prior) ? step.prior : []).filter(n => Number.isInteger(n) && n >= 0 && n < i),
  }));
  return { steps, action, recipients };
}

// What each step writes, by position: "output" alone; "planning notes" then "summary".
function stepNames(n) {
  if (n <= 1) return ['output'];
  if (n === 2) return ['planning notes', 'summary'];
  return [...Array.from({ length: n - 1 }, (_, i) => `notes ${i + 1}`), 'summary'];
}

// The `run` request for a run: earlier steps first, then sources; check the last step.
function runRequest(policy, run) {
  const names = stepNames(run.steps.length);
  return buildRequest('run', {
    policy,
    steps: run.steps.map((s, i) => ({ agent: s.agent, reads: [...s.prior.map(n => `step:${n}`), ...s.sources.map(x => `source:${x}`)], writes: names[i] })),
    check: { step: run.steps.length - 1, action: run.action, recipients: Array.isArray(run.recipients) ? run.recipients : null },
  });
}

// One quiet line per step: what the engine recorded, or why it refused.
function recordedLine(step) {
  if (!step) return { cls: 'recorded muted', text: 'not run' };
  if (step.rejected) return { cls: 'recorded refused', text: `refused — ${String(step.rejected.message).replace(/`/g, '')}` };
  const a = step.artifact || {};
  return { cls: 'recorded', text: `${step.writes} — carries: ${names(a.sources)} — readable by: ${names(a.readers)}` };
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

/* ---------- DOM building: plain text, no graphics ---------- */

function h(tag, props, ...children) {
  const el = document.createElement(tag);
  applyProps(el, props);
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false) continue;
    el.append(typeof child === 'object' ? child : String(child));
  }
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

// Turn `backticked` names in engine text into <code>.
function richText(text) {
  return String(text ?? '').split('`').map((part, i) => (i % 2 ? h('code', { text: part }) : part)).filter(p => p !== '');
}

const capitalise = text => String(text).charAt(0).toUpperCase() + String(text).slice(1);
const names = values => list(values).join(', ') || 'no one';

// "path — message (did you mean `x`?)"; clicking one moves the editor to that line.
function renderDiagnostics(diagnostics, onPick) {
  return (Array.isArray(diagnostics) ? diagnostics : []).map(d => {
    const severity = d.severity === 'warning' ? 'warning' : 'error';
    return h('li', null, h('button', { class: `diag ${severity}`, type: 'button', on: { click: () => onPick && onPick(d) } },
      severity === 'warning' ? 'warning: ' : '',
      d.path ? `${d.path} — ` : '',
      richText(d.message),
      d.suggestion ? [' (', richText(d.suggestion), ')'] : null));
  });
}

function renderTests(tests) {
  if (!Array.isArray(tests)) return [h('li', { class: 'muted', text: 'Fix the errors above to run the tests.' })];
  if (!tests.length) return [h('li', { class: 'muted', text: 'This policy has no tests.' })];
  return tests.map(t => {
    const pass = Boolean(t.pass);
    const got = t.outcome && VERDICTS[t.outcome.verdict];
    const expected = VERDICTS[t.expect];
    return h('li', { class: pass ? 'pass' : 'fail' }, h('span', { class: 'tag', text: pass ? 'PASS' : 'FAIL' }), ' ', t.name,
      pass ? null : h('span', { class: 'why', text: ` — expected ${expected ? expected.word : t.expect}, got ${got ? got.word : 'no decision'}` }));
  });
}

function renderExplain(text) {
  const lines = String(text || '').split('\n').map(l => l.trim()).filter(Boolean);
  if (!lines.length) return [h('li', { class: 'muted', text: 'Appears once the policy is valid.' })];
  return lines.map(line => h('li', null, richText(line)));
}

const DIFF_TAGS = { widens: 'WIDENS', narrows: 'NARROWS', neutral: 'CHANGE' };

function renderDiff(result) {
  if (!result) return [h('li', { class: 'muted', text: 'No changes from the example as loaded.' })];
  if (!result.ok && result.file) {
    return [h('li', { class: 'muted', text: result.file === 'old' ? 'The original policy could not be read.' : 'Fix the errors in the policy to compare it with the original.' })];
  }
  const changes = Array.isArray(result.changes) ? result.changes : [];
  if (!changes.length) return [h('li', { class: 'muted', text: 'No change in who can read or receive anything.' })];
  const items = changes.map(c => {
    const kind = hasOwn(DIFF_TAGS, c.kind) ? c.kind : 'neutral';
    return h('li', { class: kind }, h('span', { class: 'tag', text: DIFF_TAGS[kind] }), ' ', `${c.subject}: ${c.summary}`);
  });
  if (result.widens) items.push(h('li', { class: 'alert', text: 'This gives more people access. A person should approve each WIDENS line.' }));
  return items;
}

// The decision as text: verdict word, reason, what held it back, who may read it.
function renderDecision(outcome, context) {
  const info = VERDICTS[outcome && outcome.verdict];
  if (!info) return [h('p', { class: 'muted', text: 'The engine did not return a verdict.' })];
  const out = [
    h('p', { class: `verdict v-${info.cls}`, text: info.label }),
    h('p', null, richText(capitalise(outcome.reason || ''))),
  ];
  for (const b of Array.isArray(outcome.blocking) ? outcome.blocking : []) {
    out.push(h('p', null, 'Held back by: ', h('code', { text: b.source }), ` (${names(b.readers)} only) — leaves out ${names(b.excluded)}`));
  }
  out.push(h('p', null, `May read this output: ${names(outcome.readers)}`));
  const approval = context && context.approval;
  if (outcome.verdict === 'needs_approval' && approval) {
    const lasts = humanDuration(approval.valid_for_seconds);
    out.push(h('p', null, `Approval: one of ${names(approval.reviewers)} (${names(approval.people)}) approves this exact output${lasts ? `, valid ${lasts}` : ''}.`));
  }
  return out;
}

const num = n => Number(n || 0).toLocaleString('en-US');

// The CLI's own summary line: "8 runs, 972 events: 111 allow, 420 deny, 36 needs_approval — 0 violations".
function simulationLine(result) {
  const d = result.decisions || {};
  const v = Array.isArray(result.violations) ? result.violations.length : 0;
  return `${num(result.count)} ${Number(result.count) === 1 ? 'run' : 'runs'}, ${num(result.events)} events: ${num(d.allow)} allow, ${num(d.deny)} deny, ${num(d.needs_approval)} needs_approval — ${plural(v, 'violation')}`;
}

function renderSimulation(result) {
  const errors = (Array.isArray(result.diagnostics) ? result.diagnostics : []).filter(d => d && d.severity === 'error');
  if (!result.ok && errors.length) {
    return [
      h('p', { class: 'alert', text: 'The simulator only runs a policy that is valid and passes its own tests:' }),
      h('ul', null, errors.map(d => h('li', null, richText(d.message)))),
    ];
  }
  const violations = Array.isArray(result.violations) ? result.violations : [];
  const out = [h('pre', { text: simulationLine(result) })];
  if (violations.length) {
    out.push(h('div', { role: 'alert' },
      h('p', { class: 'alert', text: `${plural(violations.length, 'safety invariant violation')}. This is a bug in TrustGraph; please report it with the seed.` }),
      h('ul', null, violations.map(v => h('li', null, `${invariantLabel(v.invariant)} (seed ${v.seed}, step ${v.step}): ${v.detail}`)))));
  }
  const coverage = result.coverage && typeof result.coverage === 'object' ? result.coverage : {};
  const keys = [...Object.keys(COVERAGE_LABELS).filter(k => hasOwn(coverage, k)), ...Object.keys(coverage).filter(k => !hasOwn(COVERAGE_LABELS, k)).sort()];
  if (keys.length) {
    const hit = keys.filter(k => Number(coverage[k]) > 0).length;
    out.push(h('details', null,
      h('summary', { text: `Paths exercised: ${hit} of ${keys.length}` }),
      h('ul', { class: 'lines' }, keys.map(k => {
        const n = Number(coverage[k]) || 0;
        return h('li', { class: n ? 'hit' : 'miss' }, h('span', { class: 'mark', 'aria-hidden': 'true', text: n ? '✓ ' : '· ' }),
          coverageLabel(k), ` ×${n}`, h('span', { class: 'sr-only', text: n ? ' (exercised)' : ' (not exercised)' }));
      }))));
  }
  return out;
}

/* ---------- experiments (per example) ---------- */

const HANDOFF_RUN = {
  steps: [
    { agent: 'researcher', sources: ['salaries'], prior: [] },
    { agent: 'planner', sources: ['roadmap'], prior: [0] },
  ],
  action: 'post_to_engineering',
  recipients: null,
};
const STARTER_LEAK = toRun({ agent: 'planner', sources: ['salaries', 'roadmap'], action: 'post_to_slack' });

const EXPERIMENTS = {
  handoff: [
    { id: 'launder', label: 'Launder the salary file through notes', run: HANDOFF_RUN,
      note: 'Two agents, two steps: salaries → planning notes → a summary for engineering.' },
    { id: 'uncleared', label: "Hand the notes to an agent that isn't cleared", run: HANDOFF_RUN,
      edit: [/(\n {2}salaries:[^\n]*\n {4}readers: \[hr\]\n {4}agents: \[researcher), planner\]/, '$1]'],
      note: 'Removed `planner` from salaries.agents: it may not take in anything built on salaries.' },
    { id: 'roadmap', label: 'Drop the notes, keep the roadmap',
      run: { ...HANDOFF_RUN, steps: [HANDOFF_RUN.steps[0], { agent: 'planner', sources: ['roadmap'], prior: [] }] },
      note: 'planner wrote the summary from the roadmap alone.' },
    { id: 'hr', label: 'Send the same summary to HR instead', run: { ...HANDOFF_RUN, action: 'brief_hr' },
      note: 'Same summary, sent to hr with `brief_hr`.' },
    { id: 'widen', label: 'Quietly widen the salary file', run: HANDOFF_RUN,
      edit: [/(\n {2}salaries:[^\n]*\n {4}readers: \[)hr\]/, '$1hr, engineering]'],
      note: 'Added `engineering` to salaries.readers. A test fails and the diff flags it.' },
  ],
  starter: [
    { id: 'leak', label: 'Leak the salary file', run: STARTER_LEAK,
      note: 'planner read salaries and roadmap and posts the result to engineering.' },
    { id: 'roadmap', label: 'Post only the roadmap', run: toRun({ agent: 'planner', sources: ['roadmap'], action: 'post_to_slack' }),
      note: 'planner read only the roadmap and posts it to engineering.' },
    { id: 'email', label: 'Email a customer', run: toRun({ agent: 'mailer', sources: ['price_sheet'], action: 'email_customer' }),
      note: 'mailer read the price sheet and emails the customer.' },
    { id: 'widen', label: 'Give engineering the salary file', run: STARTER_LEAK,
      edit: [/(\n {2}salaries:\n {4}readers: \[)([^\]\n]*)\]/, '$1$2, engineering]'],
      note: 'Added `engineering` to salaries.readers. A test now fails and the diff flags it.' },
    { id: 'typo', label: 'Misspell a group', run: STARTER_LEAK,
      edit: [/(\n {2}roadmap:\n {4}readers: \[hr, )engineering\]/, '$1enginering]'],
      note: 'Changed `engineering` to `enginering` in roadmap.readers.' },
  ],
};

// The policy text and run an experiment produces from the original, or null if it does not apply.
function applyExperiment(exampleId, id, original) {
  const experiment = (EXPERIMENTS[exampleId] || []).find(e => e.id === id);
  if (!experiment) return null;
  let policy = String(original ?? '');
  if (experiment.edit) {
    const [pattern, replacement] = experiment.edit;
    if (!pattern.test(policy)) return null;
    policy = policy.replace(pattern, replacement);
  }
  return { policy, run: cloneRun(experiment.run), note: experiment.note };
}

// The starter without its comment lines: the same policy, shorter to read.
function stripComments(text) {
  return String(text).split('\n').filter(line => !/^\s*#/.test(line)).join('\n').replace(/\n{3,}/g, '\n\n').replace(/^\n+/, '');
}

// The hand-off first, then the comment-free starter and its annotated original, then the rest.
function arrangeExamples(examples) {
  const starter = examples.find(e => e.id === STARTER);
  const handoff = examples.find(e => e.id === DEFAULT_EXAMPLE);
  const rest = examples.filter(e => e !== starter && e !== handoff);
  return [
    ...(handoff ? [handoff] : []),
    ...(starter ? [
      { ...starter, policy: stripComments(starter.policy) },
      { ...starter, id: `${STARTER}-annotated`, title: `${starter.title || 'Starter'} (annotated)` },
    ] : []),
    ...rest,
  ];
}

/* ---------- the live page ---------- */

const ui = { recorded: [] };
const state = {
  examples: [],
  exampleId: null,
  original: null,
  summary: null,
  run: null,
  valid: false,
  escaped: false,
  timer: null,
  experiment: null,
};
const lanes = { examples: lane(), check: lane(), decide: lane(), diff: lane(), simulate: lane() };

function $(id) { return document.getElementById(id); }

function setStatus(cls, text) {
  ui.status.setAttribute('class', `status ${cls}`);
  ui.status.textContent = text;
}

function showDecisionMessage(title, detail, cls) {
  ui.decision.replaceChildren(h('p', { class: cls ? `verdict ${cls}` : 'verdict', text: title }), detail ? h('p', { class: 'muted', text: detail }) : null);
}

// Experiments for the selected example; the hand-off's when examples failed to load.
function experimentSet() {
  return EXPERIMENTS[state.exampleId ?? (state.original != null ? DEFAULT_EXAMPLE : '')] || null;
}

function renderExperiments() {
  if (!ui.experiments) return;
  const set = experimentSet();
  ui.experiments.hidden = !set;
  ui.experimentButtons.replaceChildren(...(set || []).map(e => h('button', {
    type: 'button', class: 'exp', 'aria-pressed': String(state.experiment === e.id), on: { click: () => runExperiment(e.id) },
  }, e.label)));
  const current = (set || []).find(e => e.id === state.experiment);
  ui.experimentNote.replaceChildren(...(current ? richText(current.note) : []));
}

function clearExperiment() {
  if (state.experiment === null) return;
  state.experiment = null;
  renderExperiments();
}

async function runExperiment(id) {
  const result = applyExperiment(state.exampleId ?? DEFAULT_EXAMPLE, id, state.original);
  if (!result) return;
  state.experiment = id;
  ui.policy.value = result.policy;
  state.run = result.run;
  renderExperiments();
  await runCheck();
}

async function loadExamples() {
  setStatus('pending', 'Loading examples…');
  const res = await lanes.examples.run(buildRequest('examples'));
  if (!res.fresh) return;
  const examples = res.value && Array.isArray(res.value.examples) ? res.value.examples : [];
  if (res.error || !examples.length) {
    state.original = ui.policy.value;
    setStatus('offline', `Examples unavailable: ${res.error ? res.error.message : 'none returned'}`);
    renderExperiments();
    await runCheck();
    return;
  }
  state.examples = arrangeExamples(examples);
  ui.example.replaceChildren(...state.examples.map(e => h('option', { value: e.id, text: e.title || e.id })));
  ui.example.disabled = false;
  await loadExample(state.examples[0].id);
}

async function loadExample(id) {
  const example = state.examples.find(e => e.id === id);
  if (!example) return;
  state.exampleId = id;
  ui.example.value = id;
  ui.policy.value = example.policy;
  state.original = example.policy;
  state.run = null;
  state.experiment = null;
  lanes.simulate.cancel();
  ui.simResult.replaceChildren(h('p', { class: 'muted', text: 'Uses the policy in the playground above.' }));
  ui.simRun.disabled = false;
  const set = EXPERIMENTS[id];
  if (set) {
    await runExperiment(set[0].id);
    return;
  }
  renderExperiments();
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
    showDecisionMessage('Engine unavailable', 'The answer will appear when the policy engine responds. Edit the policy to try again.');
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
    renderSteps();
    runSteps();
  } else {
    lanes.decide.cancel();
    showDecisionMessage('Policy has errors', 'Fix the errors in the policy to see an answer.');
  }
  runDiff(policy, Boolean(result.ok));
}

function showRecorded(results) {
  ui.recorded.forEach((line, i) => {
    const r = results ? recordedLine(results[i]) : { cls: 'recorded muted', text: '' };
    line.setAttribute('class', `${r.cls} mono`);
    line.textContent = r.text;
  });
}

// Drive the steps through a fresh engine on the server, then show what each
// step recorded and the answer for the last one.
async function runSteps() {
  if (!state.valid || !state.run) return;
  const run = state.run;
  if (run.steps.some(s => !s.sources.length && !s.prior.length)) {
    lanes.decide.cancel();
    showRecorded(null);
    showDecisionMessage('Nothing read', 'Tick at least one input for every step.');
    return;
  }
  if (Array.isArray(run.recipients) && !run.recipients.length) {
    lanes.decide.cancel();
    showDecisionMessage('No recipients', 'Tick at least one recipient.');
    return;
  }
  ui.decision.setAttribute('aria-busy', 'true');
  const res = await lanes.decide.run(runRequest(ui.policy.value, run));
  if (!res.fresh) return;
  ui.decision.setAttribute('aria-busy', 'false');
  if (res.error) { showDecisionMessage('Engine unavailable', res.error.message); return; }
  const value = res.value;
  if (!value.ok) {
    showRecorded(null);
    const first = (value.diagnostics || [])[0];
    showDecisionMessage('Policy has errors', first ? first.message : 'Fix the errors in the policy to see an answer.');
    return;
  }
  const steps = Array.isArray(value.steps) ? value.steps : [];
  showRecorded(steps);
  const refused = steps.findIndex(s => s && s.rejected);
  if (refused >= 0) {
    showDecisionMessage('No answer', `The engine refused step ${refused + 1}, so the run stopped there.`, 'v-deny');
    return;
  }
  const decision = value.decision;
  if (!decision) { showDecisionMessage('No answer', 'The engine returned no decision.'); return; }
  if (decision.rejected) { showDecisionMessage('No answer', decision.rejected.message, 'v-deny'); return; }
  const action = state.summary.actions[run.action] || {};
  ui.decision.replaceChildren(...renderDecision(decision, { approval: action.approval || null }));
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
  if (res.error) { ui.diff.replaceChildren(h('li', { class: 'muted', text: `Couldn't compare: ${res.error.message}` })); return; }
  ui.diff.replaceChildren(...renderDiff(res.value));
}

function chip(value, label, checked, onChange, title) {
  return h('label', { class: 'chip', title },
    h('input', { type: 'checkbox', value, checked, on: { change: event => onChange(event.target.checked) } }), label);
}

function select(id, values, selected, onChange) {
  const el = h('select', { id, name: id, on: { change: event => onChange(event.target.value) } }, values.map(v => h('option', { value: v, text: v })));
  el.value = selected;
  return el;
}

// Each step as an editable sentence, with the line the engine recorded under it.
function renderSteps(focusId) {
  const summary = state.summary;
  const run = state.run;
  const labels = stepNames(run.steps.length);
  const many = run.steps.length > 1;
  ui.recorded = [];
  const blocks = run.steps.map((step, i) => {
    const last = i === run.steps.length - 1;
    const toggle = (array, item) => on => {
      const at = array.indexOf(item);
      if (on && at < 0) array.push(item);
      if (!on && at >= 0) array.splice(at, 1);
      clearExperiment();
      runSteps();
    };
    const inputs = [
      ...Array.from({ length: i }, (_, n) => chip(`step:${n}`, labels[n], step.prior.includes(n), toggle(step.prior, n))),
      ...Object.entries(summary.sources || {}).map(([name, s]) =>
        chip(name, name, step.sources.includes(name), toggle(step.sources, name), list(s.agents).includes(step.agent) ? null : `${step.agent} may not use ${name}`)),
    ];
    const agentId = `agent-${i}`;
    const tail = last
      ? [' and wants to ', h('label', { class: 'sr-only', for: 'action', text: 'Action' }),
        select('action', Object.keys(summary.actions || {}), run.action, value => { run.action = value; run.recipients = null; clearExperiment(); renderSteps('action'); runSteps(); }),
        h('span', { 'aria-hidden': 'true', text: '→' }), h('span', { class: 'sr-only', text: ' sending to ' }),
        h('span', { class: 'chips', role: 'group', 'aria-label': 'Recipients' }, recipientChips()), '.']
      : [' and wrote ', h('b', { text: labels[i] }), '.'];
    const recorded = h('p', { class: 'recorded mono' });
    ui.recorded.push(recorded);
    return h('div', { class: 'step' },
      h('p', { class: 'sentence' },
        many ? h('span', { class: 'step-n mono', text: `${i + 1}.` }) : null,
        h('label', { class: 'sr-only', for: agentId, text: `Agent for step ${i + 1}` }),
        select(agentId, list(summary.agents), step.agent, value => { step.agent = value; clearExperiment(); renderSteps(agentId); runSteps(); }),
        ' read ', h('span', { class: 'chips', role: 'group', 'aria-label': `What step ${i + 1} read` }, inputs), tail),
      recorded);
  });
  const controls = h('p', { class: 'step-controls' },
    run.steps.length < MAX_UI_STEPS ? h('button', { class: 'plain', type: 'button', on: { click: addStep } }, '+ add a step') : null,
    many ? h('button', { class: 'plain', type: 'button', on: { click: removeStep } }, '− remove the last step') : null);
  ui.steps.replaceChildren(...blocks, controls);
  if (focusId) { const el = $(focusId); if (el && typeof el.focus === 'function') el.focus(); }
}

function recipientChips() {
  const run = state.run;
  const action = state.summary.actions[run.action] || {};
  const people = list(action.people);
  const chosen = expandRecipients(run, state.summary);
  return people.map(person => chip(person, person, chosen.includes(person), on => {
    const set = new Set(expandRecipients(run, state.summary));
    if (on) set.add(person); else set.delete(person);
    const picked = people.filter(p => set.has(p));
    run.recipients = picked.length === people.length ? null : picked;
    clearExperiment();
    runSteps();
  }));
}

// A new last step reads the previous one; its agent starts as the previous agent.
function addStep() {
  const run = state.run;
  if (run.steps.length >= MAX_UI_STEPS) return;
  const previous = run.steps[run.steps.length - 1];
  run.steps.push({ agent: previous.agent, sources: [], prior: [run.steps.length - 1] });
  clearExperiment();
  renderSteps(`agent-${run.steps.length - 1}`);
  runSteps();
}

function removeStep() {
  const run = state.run;
  if (run.steps.length <= 1) return;
  run.steps.pop();
  clearExperiment();
  renderSteps(`agent-${run.steps.length - 1}`);
  runSteps();
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
    ui.simResult.replaceChildren(h('p', { class: 'alert', text: 'Enter a whole number from 0 to 65535 as the seed.' }));
    return;
  }
  ui.simRun.disabled = true;
  ui.simRun.textContent = 'Running…';
  ui.simResult.setAttribute('aria-busy', 'true');
  try {
    const res = await lanes.simulate.run(buildRequest('simulate', { policy: ui.policy.value, seed }));
    if (!res.fresh) return;
    if (res.error) ui.simResult.replaceChildren(h('p', { class: 'alert', text: `The simulator could not run: ${res.error.message}` }));
    else ui.simResult.replaceChildren(...renderSimulation(res.value));
  } finally {
    ui.simRun.disabled = false;
    ui.simRun.textContent = 'Run';
    ui.simResult.setAttribute('aria-busy', 'false');
  }
}

function boot() {
  Object.assign(ui, {
    example: $('example'), policy: $('policy'), status: $('check-status'), reset: $('reset'),
    diagnostics: $('diagnostics'), tests: $('tests'), steps: $('steps'), decision: $('decision'), explain: $('explain'), diff: $('diff'),
    experiments: $('experiments'), experimentButtons: $('experiment-buttons'), experimentNote: $('experiment-note'),
    simForm: $('sim-form'), seed: $('sim-seed'), simRun: $('sim-run'), simResult: $('sim-result'),
  });
  if (!ui.policy) return;
  ui.policy.addEventListener('input', () => { clearExperiment(); scheduleCheck(); });
  ui.policy.addEventListener('keydown', onEditorKey);
  ui.example.addEventListener('change', () => loadExample(ui.example.value));
  ui.reset.addEventListener('click', () => {
    if (state.original == null) return;
    const set = experimentSet();
    if (set) { runExperiment(set[0].id); return; }
    ui.policy.value = state.original;
    state.run = null;
    clearExperiment();
    runCheck();
  });
  ui.simForm.addEventListener('submit', runSimulation);
  renderExperiments();
  return loadExamples();
}

if (!globalThis.TRUSTGRAPH_NO_BOOT) boot();
