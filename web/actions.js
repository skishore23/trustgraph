const notice = document.querySelector('#notice');
const items = document.querySelector('#items');
let working = false;
const statuses = { PERMISSION_REVOKED: 'BLOCKED · PERMISSION REVOKED', NEEDS_APPROVAL: 'NEEDS HUMAN REVIEW', FRESH_APPROVAL_REQUIRED: 'PREVIOUS APPROVAL IS STALE', READY_TO_CREATE: 'APPROVED · READY TO CREATE' };
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
  for (const button of document.querySelectorAll('button')) button.disabled = true;
  notice.textContent = verb === 'execute' ? 'Recording the result. If the response is lost, inspect status before trying again.' : 'Recording approval for this version…';
  try {
    await api(`/api/actions/${encodeURIComponent(item.id)}/${verb}`, { method: 'POST', body: JSON.stringify({ revision: item.revision, context_digest: item.context_digest }) });
    notice.textContent = '';
  } catch (error) { notice.textContent = error.message; }
  finally { working = false; await refresh(); }
}
async function permissionChange(permission) {
  working = true;
  for (const button of document.querySelectorAll('button')) button.disabled = true;
  try {
    await api('/api/publication-permission', { method: 'POST', body: JSON.stringify({ granted: !permission.granted, revision: permission.revision }) });
    notice.textContent = permission.granted
      ? 'Permission revoked. Creation is blocked and earlier approvals are stale.'
      : 'Permission granted. Review and approve the exact issue before creating it.';
  } catch (error) { notice.textContent = error.message; }
  finally { working = false; await refresh(); }
}
function renderPermission(permission) {
  const section = document.querySelector('#permission');
  section.className = permission.granted ? 'permission granted' : 'permission revoked';
  const badge = element('span', permission.granted ? 'GRANTED' : 'REVOKED', 'permission-badge');
  const title = element('h2', 'GitHub publication permission');
  const description = element('p', `Create issues in ${permission.repository}`);
  const explanation = element('p', permission.granted
    ? 'Permission allows this gateway to publish. Each issue still needs exact human approval.'
    : 'This gateway cannot create issues. Restoring permission will require fresh approval.');
  const button = element('button', permission.granted ? 'Revoke permission' : 'Grant permission', permission.granted ? 'danger' : '');
  button.addEventListener('click', () => permissionChange(permission));
  section.replaceChildren(badge, title, description, explanation, button,
    element('small', `Policy revision ${permission.revision} · Controls TrustGraph publication, not GitHub account access.`));
}
function render(item) {
  const card = element('article');
  const proposal = item.action.proposal;
  const operation = item.operation;
  const meta = element('div', undefined, 'meta');
  meta.append(element('span', `${proposal.work_order.id} · revision ${item.revision}`));
  meta.append(element('span', operation ? operation.state : statuses[item.status] || item.status, 'state'));
  card.append(meta, element('h2', proposal.title), element('div', `GitHub → ${item.action.repository}`, 'destination'), element('pre', proposal.body || '(No description)'));
  const approve = element('button', 'Approve exact issue', 'secondary');
  approve.disabled = Boolean(operation) || item.approved || !item.publication_allowed;
  approve.addEventListener('click', () => action(item, 'approve'));
  const execute = element('button', item.mode === 'preview' ? 'Record approved preview' : 'Create GitHub issue');
  execute.disabled = !item.can_execute;
  execute.addEventListener('click', () => action(item, 'execute'));
  card.append(approve, execute);
  if (!operation && !item.publication_allowed) {
    card.append(element('p', 'Blocked by publication permission. Human approval cannot bypass this gate.', 'blocked'));
    const probe = element('button', 'Try blocked publication', 'secondary');
    probe.addEventListener('click', () => action(item, 'execute'));
    card.append(probe);
  } else if (!operation && item.status === 'FRESH_APPROVAL_REQUIRED') {
    card.append(element('p', 'Permission or review context changed. The earlier approval cannot be reused.', 'blocked'));
  }
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
    renderPermission(state.publication_permission);
    items.replaceChildren(...state.items.map(render));
    if (!state.items.length) items.append(element('p', 'Waiting for an issue proposal from the 8090 agent.', 'empty'));
  } catch (error) { notice.textContent = error.message; }
  finally { document.querySelector('#import-form button').disabled = false; }
}
async function start() {
  const token = location.hash.slice(1);
  history.replaceState(null, '', location.pathname);
  if (/^[a-f0-9]{64}$/i.test(token)) {
    try { await api('/api/session', { method: 'POST', headers: { Authorization: `Bearer ${token}` } }); }
    catch (error) { notice.textContent = error.message; }
  }
  await refresh();
  document.querySelector('#import-form').addEventListener('submit', async event => {
    event.preventDefault();
    if (working) return;
    working = true;
    for (const button of document.querySelectorAll('button')) button.disabled = true;
    try {
      const text = document.querySelector('#proposal-json').value.trim();
      const fenced = text.match(/^```(?:json)?\s*\n([\s\S]*?)\n```$/i);
      const proposal = JSON.parse(fenced ? fenced[1] : text);
      await api('/api/proposals/import', { method: 'POST', body: JSON.stringify(proposal) });
      notice.textContent = 'Proposal imported for review. No issue was created.';
      document.querySelector('#proposal-json').value = '';
    } catch (error) { notice.textContent = error.message; }
    finally { working = false; await refresh(); document.querySelector('#import-form button').disabled = false; }
  });
  setInterval(refresh, 4000);
}
start();
