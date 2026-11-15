// Verify hosted progress survives requests and refreshes without server sessions.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('web/story.js', 'utf8').split('function notify')[0];
function client(hosted = true, saved = '[]') {
  const storage = new Map([['trustgraph:demo:v1', saved]]);
  const calls = [];
  let ok = true;
  const queued = [];
  const context = vm.createContext({
    document: { querySelector: selector => selector.startsWith('meta') ? { content: hosted ? 'hosted' : '' } : {} },
    sessionStorage: { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value), removeItem: key => storage.delete(key) },
    fetch: async (url, options) => {
      calls.push({ url, ...options, body: options.body && JSON.parse(options.body) });
      if (queued.length) return queued.shift();
      const body = JSON.stringify(ok ? { simulated: true } : { error: 'Rejected' });
      return { ok, status: ok ? 200 : 409, text: async () => body };
    },
  });
  vm.runInContext(source, context);
  // Queue a raw response (e.g. a plain-text Axum rejection) for the next fetch.
  const respond = (status, text) => queued.push({ ok: status < 400, status, text: async () => text });
  return { calls, storage, context, respond, fail: () => { ok = false; }, succeed: () => { ok = true; } };
}
async function main() {
  const browser = client();
  const command = { action: 'generate', approved_inputs: false, expected_revision: 1 };
  browser.context.commandInput = command;
  await vm.runInContext('api("", commandInput)', browser.context);
  assert.equal(browser.calls[0].url, '/api/demo');
  assert.deepEqual(browser.calls[0].body, { history: [], command });
  assert.deepEqual(JSON.parse(browser.storage.get('trustgraph:demo:v1')), [command]);
  const refreshed = client(true, browser.storage.get('trustgraph:demo:v1'));
  await vm.runInContext('api()', refreshed.context);
  assert.deepEqual(refreshed.calls[0].body, { history: [command] });
  await vm.runInContext('api("/compare", {policy:{}, scenario:{}})', browser.context);
  assert.equal(JSON.parse(browser.storage.get('trustgraph:demo:v1')).length, 1);
  browser.fail();
  await assert.rejects(vm.runInContext('api("", commandInput)', browser.context), /Rejected/);
  assert.equal(JSON.parse(browser.storage.get('trustgraph:demo:v1')).length, 1);
  browser.succeed();
  await vm.runInContext('api("/reset", {})', browser.context);
  assert.deepEqual(browser.calls.at(-1).body, { history: [] });
  assert.equal(browser.storage.get('trustgraph:demo:v1'), '[]');

  // Non-JSON error bodies surface as readable messages, not "Unexpected token".
  const plain = client();
  plain.respond(422, 'Failed to deserialize the JSON body into the target type');
  await assert.rejects(vm.runInContext('api()', plain.context), error => error.status === 422 && /422: Failed to deserialize/.test(error.message));
  plain.respond(200, '<html>proxy page</html>');
  await assert.rejects(vm.runInContext('api()', plain.context), /unreadable response/);
  plain.respond(400, JSON.stringify({ error: 'Choose one demo operation per request.' }));
  await assert.rejects(vm.runInContext('api()', plain.context), /^Error: Choose one demo operation/);

  // A stale saved history that the server rejects is cleared and retried once with [].
  const stale = client(true, JSON.stringify([command, command]));
  stale.respond(422, 'unknown variant');
  const loaded = await vm.runInContext('loadInitialState()', stale.context);
  assert.equal(loaded.restarted, true);
  assert.equal(JSON.stringify(loaded.value), JSON.stringify({ simulated: true }));
  assert.equal(JSON.stringify(stale.calls.map(call => call.body.history.length)), "[2,0]");
  assert.equal(stale.storage.get('trustgraph:demo:v1'), '[]');
  // Server outages (5xx) and empty histories are not retried.
  const outage = client(true, JSON.stringify([command]));
  outage.respond(503, 'Service Unavailable');
  await assert.rejects(vm.runInContext('loadInitialState()', outage.context), /503/);
  assert.equal(outage.calls.length, 1);
  const fresh = client(true, '[]');
  fresh.respond(400, JSON.stringify({ error: 'bad' }));
  await assert.rejects(vm.runInContext('loadInitialState()', fresh.context), /bad/);
  assert.equal(fresh.calls.length, 1);

  // Minutes since approval must be an integer within the advertised bounds.
  for (const valid of ['0', '15', '10080', 42]) assert.notEqual(vm.runInContext(`approvalAge(${JSON.stringify(valid)})`, plain.context), null);
  for (const invalid of ['', ' ', '-1', '1.5', '10081', 'abc', '1e9']) assert.equal(vm.runInContext(`approvalAge(${JSON.stringify(invalid)})`, plain.context), null);

  const local = client(false);
  await vm.runInContext('api()', local.context);
  assert.equal(local.calls[0].url, '/api/story');
  assert.equal(local.calls[0].method, 'GET');
  console.log('Hosted demo progress passed: replay, refresh, comparison, rejected steps, reset, non-JSON errors, stale-history recovery, numeric validation, and local compatibility.');
}
main().catch(error => { console.error(error); process.exitCode = 1; });
