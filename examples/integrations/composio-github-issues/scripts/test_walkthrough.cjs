// Exercise presenter transitions from server decisions, without a browser or writes.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require('node:path').join(__dirname, '..', 'web', 'actions.js'), 'utf8').replace(/start\(\);\s*$/, '');
const context = vm.createContext({ document: {}, localStorage: { getItem: () => null, setItem: () => {} } });
vm.runInContext(source, context);
const base = { context_digest: 'current', revision: 1, publication_allowed: true, approved: true, review_history: [{ current: true }], operation: null };
function phase(scenario, changes = {}, check = null) {
  context.input = { ...base, ...changes };
  context.scenario = scenario;
  context.check = check;
  return vm.runInContext('selected=scenario; saved={[selected]:{events:[],check}}; phase(input).kind', context);
}
assert.equal(phase('permission', { publication_allowed: false, approved: false, review_history: [] }), 'grant');
assert.equal(phase('permission', { approved: false, review_history: [] }), 'review');
assert.equal(phase('permission'), 'check');
const checked = { context_digest: 'current', approved: true, publication_allowed: true, allowed: true };
assert.equal(phase('permission', {}, checked), 'publish');
assert.equal(phase('permission', { approved: false }, checked), 'review_again', 'expired review cannot reuse a cached passing check');
assert.equal(phase('permission', {}, { ...checked, approved: false, allowed: false }), 'check', 'a check performed before human approval must be repeated');
assert.equal(phase('revocation'), 'revoke');
assert.equal(phase('revocation', { publication_allowed: false, approved: false, review_history: [{ current: false }] }), 'revoked');
assert.equal(phase('revocation', { publication_allowed: false, approved: false, review_history: [{ current: false }] }, { context_digest: 'current', publication_allowed: false, approved: false, allowed: false }), 'restore');
assert.equal(phase('revocation', { approved: false, review_history: [{ current: false }] }), 'review_again');
assert.equal(phase('revocation', { review_history: [{ current: false }, { current: true }] }), 'check');
assert.equal(phase('revision'), 'edit');
assert.equal(phase('revision', { revision: 2, approved: false, review_history: [{ current: false }] }), 'edited');
assert.equal(phase('revision', { revision: 2, approved: false, review_history: [{ current: false }] }, { context_digest: 'current', publication_allowed: true, approved: false, allowed: false }), 'review_again');
assert.equal(phase('revision', { revision: 2 }), 'check');
assert.equal(phase('permission', { operation: { state: 'OUTCOME_UNKNOWN' } }), 'uncertain');
assert.equal(phase('permission', { operation: { state: 'PUBLISHED' } }), 'published');
console.log('Walkthrough transitions passed: permission, review, revocation, edits, cached checks, and uncertain outcomes.');
