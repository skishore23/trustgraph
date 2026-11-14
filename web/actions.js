const notice = document.querySelector('#notice');
const items = document.querySelector('#items');
let working = false;
const element = (tag, text, className) => {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
};
async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { 'Content-Type': 'application/json', ...options.headers } });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error || 'Open the private operator sign-in file printed at startup.');
  return value;
}
async function action(item, verb) {
  working = true;
  for (const button of items.querySelectorAll('button')) button.disabled = true;
  notice.textContent = verb === 'execute' ? 'Recording the result. If the response is lost, inspect status before trying again.' : 'Recording approval for this version…';
  try {
    await api(`/api/actions/${encodeURIComponent(item.id)}/${verb}`, { method: 'POST', body: JSON.stringify({ revision: item.revision, context_digest: item.context_digest }) });
    notice.textContent = '';
  } catch (error) { notice.textContent = error.message; }
  finally { working = false; await refresh(); }
}
function render(item) {
  const card = element('article');
  const proposal = item.action.proposal;
  const operation = item.operation;
  const meta = element('div', undefined, 'meta');
  meta.append(element('span', `${proposal.work_order.id} · revision ${item.revision}`));
  meta.append(element('span', operation ? operation.state : item.approved ? 'APPROVED' : 'NEEDS APPROVAL'));
  card.append(meta, element('h2', proposal.title), element('div', `GitHub → ${item.action.repository}`, 'destination'), element('pre', proposal.body || '(No description)'));
  const approve = element('button', 'Approve exact issue', 'secondary');
  approve.disabled = Boolean(operation) || item.approved;
  approve.addEventListener('click', () => action(item, 'approve'));
  const execute = element('button', item.mode === 'preview' ? 'Record approved preview' : 'Create GitHub issue');
  execute.disabled = Boolean(operation) || !item.approved;
  execute.addEventListener('click', () => action(item, 'execute'));
  card.append(approve, execute);
  if (operation) {
    let text = operation.state === 'OUTCOME_UNKNOWN' ? 'Outcome unknown. Inspect GitHub and the provider logs; this action will not be automatically resent.' : operation.state;
    if (operation.receipt?.simulated) text = 'Preview recorded. No GitHub issue was created.';
    const result = element('p', text, 'result');
    const url = operation.receipt?.provider_id;
    if (url?.startsWith('https://github.com/')) {
      const link = element('a', 'Open created issue'); link.href = url; link.rel = 'noreferrer';
      result.append(document.createTextNode(' · '), link);
    }
    card.append(result);
  }
  const details = element('details');
  details.append(element('summary', 'Exact action and approval binding'), element('pre', JSON.stringify({ action: item.action, context_digest: item.context_digest }, null, 2)));
  card.append(details);
  return card;
}
async function refresh() {
  if (working) return;
  try {
    const state = await api('/api/actions');
    document.querySelector('#mode').textContent = state.mode === 'preview' ? 'PREVIEW · NO EXTERNAL WRITES' : 'LIVE · COMPOSIO';
    items.replaceChildren(...state.items.map(render));
    if (!state.items.length) items.append(element('p', 'Waiting for an issue proposal from the 8090 agent.', 'empty'));
  } catch (error) { notice.textContent = error.message; }
}
async function start() {
  const token = location.hash.slice(1);
  history.replaceState(null, '', location.pathname);
  if (/^[a-f0-9]{64}$/i.test(token)) {
    try { await api('/api/session', { method: 'POST', headers: { Authorization: `Bearer ${token}` } }); }
    catch (error) { notice.textContent = error.message; }
  }
  await refresh();
  setInterval(refresh, 4000);
}
start();
