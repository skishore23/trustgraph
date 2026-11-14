// Verify hosted progress survives requests and refreshes without server sessions.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('web/story.js', 'utf8').split('function notify')[0];
function client(hosted = true, saved = '[]') {
  const storage = new Map([['trustgraph:demo:v1', saved]]);
  const calls = [];
  let ok = true;
  const context = vm.createContext({
    document: { querySelector: selector => selector.startsWith('meta') ? { content: hosted ? 'hosted' : '' } : {} },
    sessionStorage: { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value) },
    fetch: async (url, options) => {
      calls.push({ url, ...options, body: options.body && JSON.parse(options.body) });
      return { ok, json: async () => ok ? { simulated: true } : { error: 'Rejected' } };
    },
  });
  vm.runInContext(source, context);
  return { calls, storage, context, fail: () => { ok = false; }, succeed: () => { ok = true; } };
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
  const local = client(false);
  await vm.runInContext('api()', local.context);
  assert.equal(local.calls[0].url, '/api/story');
  assert.equal(local.calls[0].method, 'GET');
  console.log('Hosted demo progress passed: replay, refresh, comparison, rejected steps, reset, and local compatibility.');
}
main().catch(error => { console.error(error); process.exitCode = 1; });
