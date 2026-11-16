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

console.log('serve.test.cjs: ok');
