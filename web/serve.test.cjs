// Logic tests for the trustgraph serve operator page (no DOM).
// Run from repository root: node web/serve.test.cjs
const assert = require('node:assert/strict');
const page = require('./serve.js');

// The lists are never re-rendered while the operator points at or works in them.
{
  let interacting = true;
  const rendered = [];
  const shown = [];
  const scheduler = page.createScheduler({
    render: (state, focusKey) => rendered.push([state.n, focusKey]),
    interacting: () => interacting,
    queued: on => shown.push(on),
  });

  assert.equal(scheduler.offer({ n: 1 }, false), false, 'held while interacting');
  assert.equal(scheduler.offer({ n: 2 }, false), false);
  assert.deepEqual(rendered, []);
  assert.equal(scheduler.hasQueued(), true);
  assert.deepEqual(shown, [true, true], 'the "updated — refresh" link shows');

  // The operator's own action (force) renders at once, focus kept.
  assert.equal(scheduler.offer({ n: 3 }, true, 'yes:approve:x'), true);
  assert.deepEqual(rendered, [[3, 'yes:approve:x']]);
  assert.equal(scheduler.hasQueued(), false);
  assert.equal(shown.at(-1), false, 'the link hides again');

  // A held update shows when asked for, newest first.
  scheduler.offer({ n: 4 }, false);
  scheduler.offer({ n: 5 }, false);
  assert.equal(scheduler.flush(), true);
  assert.deepEqual(rendered.at(-1), [5, undefined]);
  assert.equal(scheduler.flush(), false, 'nothing left to show');

  // Not interacting: updates render as they come.
  interacting = false;
  assert.equal(scheduler.offer({ n: 6 }, false), true);
  assert.deepEqual(rendered.at(-1), [6, undefined]);
}

// The approval confirmation repeats exactly what is approved.
{
  const p = {
    agent: 'mailer',
    action: 'email_customer',
    recipients: ['acme'],
    sources: ['price_sheet'],
    decision: 'c191bc63a1b2c3d4e5f6',
  };
  assert.equal(
    page.approvalSummary(p),
    'Approve: mailer may email_customer to acme, from price_sheet (decision c191bc63a1b2…).'
  );
  assert.equal(page.list([]), 'nobody');
}

// A replaced session says so; anything else asks to sign in.
{
  const replaced = { status: 401, data: { error: { code: 'operator_session_replaced' } } };
  assert.match(page.signedOutMessage(replaced), /newer operator session was started.*stop the server/);
  assert.match(page.signedOutMessage({ status: 401, data: {} }), /^Not signed in/);
}

// A confirmation for an item that is gone is dropped, so refreshing resumes.
{
  const state = {
    pending: [
      { decision: 'd1', can_approve: true },
      { decision: 'd2', can_approve: false },
    ],
    proposals: [{ proposal: 'p1' }],
  };
  assert.deepEqual(page.confirmableKeys(state), ['approve:d1', 'activate:p1']);
  assert.equal(page.staleConfirmation(state, null), false, 'nothing open');
  assert.equal(page.staleConfirmation(state, 'approve:d1'), false, 'still there');
  assert.equal(page.staleConfirmation(state, 'activate:p1'), false);
  assert.equal(page.staleConfirmation(state, 'approve:d2'), true, 'no longer approvable');
  assert.equal(page.staleConfirmation(state, 'approve:gone'), true, 'expired or evicted');
  assert.equal(page.staleConfirmation({ pending: [], proposals: [] }, 'activate:p1'), true, 'voided by an activation');
  assert.equal(page.GONE, 'That request is no longer pending.');
}

// A proposal from the policy file says who could have written it.
{
  assert.equal(
    page.proposer({ origin: 'file' }),
    'From the policy file (anyone who can edit the file could have written this)'
  );
  assert.equal(page.proposer({ origin: 'agent', from: 'mailer' }), 'Proposed by agent mailer');
}

// Gateway sessions can be ended, with a confirmation that says what is forgotten.
{
  const state = {
    pending: [],
    proposals: [],
    gateway: { mode: 'agent', problems: [], sessions: [{ session: 'agent:planner', agent: 'planner', sources: ['salaries'] }] },
  };
  assert.deepEqual(page.confirmableKeys(state), ['end:agent:planner']);
  assert.equal(page.staleConfirmation({ pending: [], proposals: [] }, 'end:agent:planner'), true, 'ended elsewhere');
  assert.equal(
    page.endSummary(state.gateway.sessions[0]),
    "End this session? planner's later actions will no longer count what it read: salaries."
  );
}

// Agent-supplied text is shown as escapes: no control, bidi or invisible
// character reaches the page as itself.
{
  assert.equal(page.neutralise('a\u001b[2Jb'), 'a\\x1b[2Jb');
  assert.equal(page.neutralise('pay\u202eboj.exe'), 'pay\\u202eboj.exe');
  assert.equal(page.neutralise('zero\u200bwidth\u2066'), 'zero\\u200bwidth\\u2066');
  assert.equal(page.neutralise('\u0000\u007f\u0085'), '\\x00\\x7f\\x85');
  assert.equal(page.neutralise('<b>bold</b>\nline\ttab'), '<b>bold</b>\nline\ttab', 'markup stays text; newline and tab kept');
  assert.equal(page.neutralise(undefined), '');
}

// A call the gateway holds shows its tool, action, recipients, every
// forwarded argument (cut with a marker) and the hash of the full arguments.
{
  const p = {
    agent: 'planner',
    action: 'brief_leadership',
    recipients: ['maya'],
    sources: ['roadmap'],
    decision: 'd'.repeat(64),
    gateway: {
      tool: 'slack__post_message',
      held: true,
      sha256: 'ab'.repeat(32),
      arguments: [
        { name: 'channel', value: '#leadership', more: 0 },
        { name: 'text', value: 'Q3\u202e plan', more: 500 },
      ],
    },
  };
  const lines = page.callLines(p);
  assert.deepEqual(lines.map(l => l.text), [
    'gateway call: slack__post_message → brief_leadership → maya',
    'forwarded arguments, exactly:',
    'channel: #leadership',
    'text: Q3\\u202e plan [… truncated — 500 more characters]',
    'SHA-256 of the full arguments: ' + 'ab'.repeat(32),
  ]);
  assert.equal(lines[2].cls, 'value');
  assert.match(page.approvalSummary(p), /as one call of slack__post_message with arguments abababababab…/);

  // After a restart the gateway no longer has it: the page says to retry.
  const gone = Object.assign({}, p, { gateway: { tool: 'slack__post_message', held: false }, can_approve: false });
  const goneLines = page.callLines(gone);
  assert.match(goneLines[1].text, /must retry it before it can be approved/);
  assert.deepEqual(page.confirmableKeys({ pending: [gone], proposals: [] }), [], 'no Approve for it');
  assert.deepEqual(page.callLines({ action: 'x' }), [], 'not a gateway call');
}

// An argument value is one row: newline, carriage return and tab show as
// escapes, a backslash is doubled, and invisible characters (soft hyphen,
// Hangul filler, tag characters, which need the u flag) are escaped too.
{
  const cp = String.fromCodePoint;
  assert.equal(page.argumentValue('a\nb\rc\td'), 'a\\nb\\rc\\td');
  assert.equal(page.argumentValue('C:\\new'), 'C:\\\\new', 'a literal backslash-n is not a newline');
  assert.equal(page.neutralise('sam' + cp(0xad) + '@corp.com'), 'sam\\xad@corp.com');
  assert.equal(page.neutralise('x' + cp(0x3164) + cp(0x115f) + cp(0xffa0)), 'x\\u3164\\u115f\\uffa0');
  assert.equal(page.neutralise('#eng' + cp(0xe0041) + cp(0xe007f)), '#eng\\u{e0041}\\u{e007f}');
  assert.equal(page.neutralise('a' + cp(0x1f600) + 'b'), 'a' + cp(0x1f600) + 'b', 'other astral text is itself');
  assert.equal(page.neutralise('x' + cp(0x2064) + cp(0x206f)), 'x\\u2064\\u206f');
}

// The arguments the matched rule constrains (who the call reaches) come
// first, by their real values, in the block and in the confirmation step.
{
  const p = {
    agent: 'planner',
    action: 'post_to_engineering',
    recipients: ['ravi', 'sam'],
    sources: ['roadmap'],
    decision: 'e'.repeat(64),
    gateway: {
      tool: 'slack__post_message',
      held: true,
      sha256: 'cd'.repeat(32),
      arguments: [
        { name: 'channel', value: '#engineering', more: 0, constrained: true },
        { name: 'text', value: 'line one\nline two', more: 0, constrained: false },
        { name: 'thread', value: '42', more: 0, constrained: false },
      ],
    },
  };
  const lines = page.callLines(p);
  assert.deepEqual(lines.map(l => l.text), [
    'gateway call: slack__post_message → post_to_engineering → ravi, sam',
    'matched by the rule, so who it reaches:',
    'channel: #engineering',
    'every other forwarded argument, exactly:',
    'text: line one\\nline two',
    'thread: 42',
    'SHA-256 of the full arguments: ' + 'cd'.repeat(32),
  ]);
  assert.deepEqual(lines.filter(l => l.cls === 'value').length, 3, 'one row per argument');
  assert.equal(
    page.approvalSummary(p),
    'Approve: planner may post_to_engineering to ravi, sam, from roadmap, as one call of slack__post_message where channel: #engineering with arguments cdcdcdcdcdcd… (decision eeeeeeeeeeee…).'
  );
}

// A blocked agent (connection mode) gets its own Clear; a blocked session
// that is listed is ended in the sessions list.
{
  const state = {
    pending: [],
    proposals: [],
    gateway: {
      mode: 'connection', problems: [],
      sessions: [{ session: 'conn:abc', agent: 'planner', sources: ['roadmap'] }],
      connections: [],
      blocked: ['agent:planner', 'conn:abc'],
    },
  };
  const entries = page.blockedEntries(state);
  assert.deepEqual(entries.map(e => [e.id, e.clearable]), [['agent:planner', true], ['conn:abc', false]]);
  assert.match(entries[0].text, /^Agent planner refused a hand-off: none of its connections, current or new, may act/);
  assert.deepEqual(page.confirmableKeys(state), ['end:conn:abc', 'clear:agent:planner']);
  assert.deepEqual(page.blockedEntries({ gateway: { sessions: [] } }), []);
}

// Open connections can be ended; an agent at the cap is named.
{
  const state = {
    pending: [],
    proposals: [],
    gateway: {
      mode: 'agent', problems: [], sessions: [],
      connections: [{ connection: 'c-1', agent: 'planner', session: 'agent:planner', started_at: 1, last_active: 2 }],
      at_cap: ['planner'],
      max_connections_per_agent: 64,
    },
  };
  assert.deepEqual(page.confirmableKeys(state), ['close:c-1']);
  assert.match(page.capNotice(state), /^planner holds the most open connections allowed \(64\); new connections are refused/);
  assert.equal(page.capNotice({ gateway: { at_cap: [] } }), '');
  assert.match(page.CONTEXT_NOTE, /agent keeps whatever is already in its own context\. Stop or restart the agent first\./);
}

console.log('serve.test.cjs: ok');
