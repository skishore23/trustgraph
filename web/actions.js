'use strict';
const $ = selector => document.querySelector(selector);
const SCENARIOS = {
  permission: { number: '01', title: 'Permission comes first.', description: 'Giving an agent a destination is not the same as approving what it sends.', steps: ['Proposal', 'Permission', 'Human review', 'Check the gate', 'Publish'], quote: 'Permission answers whether this gateway may publish. Human review answers whether this exact issue should be published.' },
  revocation: { number: '02', title: 'You can change your mind.', description: 'Approve the issue, revoke permission, and watch the earlier approval become stale.', steps: ['Proposal', 'Permission', 'Review', 'Revoke', 'Restore', 'Review again', 'Publish'], quote: 'An approval is not a permanent permission slip. Revoke access, and the gate stops. Restore access, and the old approval stays in the past.' },
  revision: { number: '03', title: 'The work can change. The review must follow.', description: 'Edit an approved proposal and see why a new version needs a fresh human decision.', steps: ['Proposal', 'Permission', 'Review', 'Edit', 'Review again', 'Publish'], quote: 'I approved a particular version. If even part of the issue changes, that old decision cannot authorize the new work.' }
};
let state = null, authenticated = false, selected = 'permission', busy = false, busyLabel = '', publishing = false;
let reviewSnapshot = null, editSnapshot = null;
let saved = {};
try { saved = JSON.parse(localStorage.getItem('trustgraph-walkthrough-v1') || '{}'); if (!saved || typeof saved !== 'object' || Array.isArray(saved)) saved = {}; } catch { saved = {}; }
if (SCENARIOS[saved.selected]) selected = saved.selected;
function persist() { saved.selected = selected; try { localStorage.setItem('trustgraph-walkthrough-v1', JSON.stringify(saved)); } catch { /* Session remains usable without storage. */ } }
function session() { if (!saved[selected] || typeof saved[selected] !== 'object') saved[selected] = { events: [] }; if (!Array.isArray(saved[selected].events)) saved[selected].events = []; return saved[selected]; }
function current() { if (!state) return null; const own = state.items.find(item => item.id === session().id && item.walkthrough === selected); return own || state.items.find(item => item.walkthrough === selected) || null; }
function node(tag, text, className) { const el = document.createElement(tag); if (text !== undefined) el.textContent = text; if (className) el.className = className; return el; }
function notify(message = '', error = false) { $('#notice').textContent = message; $('#notice').classList.toggle('error', error); }
function eventLog(text) { session().events.unshift({ text, at: new Date().toISOString() }); session().events = session().events.slice(0, 12); persist(); }
async function api(path, options = {}) {
  const response = await fetch(path, { cache: 'no-store', signal: AbortSignal.timeout(options.method ? 180000 : 15000), ...options, headers: { 'Content-Type': 'application/json', ...options.headers } });
  let text = ''; try { text = await response.text(); } catch { /* Treat an unreadable body as empty. */ }
  let value = null; try { value = text ? JSON.parse(text) : null; } catch { value = null; }
  if (!response.ok) { const error = new Error(value && typeof value.error === 'string' ? value.error : `The gateway returned ${response.status}${text ? `: ${text.slice(0, 160)}` : ''}.`); error.status = response.status; throw error; }
  if (!value || typeof value !== 'object') throw new Error('The gateway returned an unreadable response.');
  return value;
}
const post = (path, value) => api(path, { method: 'POST', body: JSON.stringify(value) });
const binding = item => ({ revision: item.revision, context_digest: item.context_digest });
function documentText(target, text) {
  target.replaceChildren(); let list = null;
  for (const line of text.split('\n')) {
    if (/^#{1,4}\s/.test(line)) { list = null; target.append(node('h4', line.replace(/^#+\s*/, ''))); }
    else if (/^[-*]\s/.test(line)) { if (!list) { list = node('ul'); target.append(list); } list.append(node('li', line.replace(/^[-*]\s*/, ''))); }
    else if (line.trim()) { list = null; target.append(node('p', line)); }
    else list = null;
  }
}
async function refresh(force = false) {
  if (busy && !force) return;
  try {
    state = await api('/api/actions'); authenticated = true; $('#signin-banner').hidden = true;
    const item = current();
    if (item) { session().id = item.id; if (item.operation) session().uncertain = false; persist(); }
    render();
  } catch (error) {
    if (error.status === 401) {
      authenticated = false; $('#signin-banner').hidden = false;
      $('#signin-banner').textContent = 'This browser needs the operator session. Open the private operator.html launch file once in this browser. Your saved walkthrough and GitHub receipts are retained.';
    } else notify('The gateway is not responding yet. Your work is saved. Refresh status before trying an action again.', true);
    render();
  }
}
async function run(label, task, isPublish = false) {
  if (busy || !authenticated) return;
  busy = true; busyLabel = label; publishing = isPublish; notify(); render();
  let result;
  try { result = await task(); }
  catch (error) {
    if (isPublish) { session().uncertain = true; persist(); eventLog('Publication response unavailable. Check the saved outcome; do not resend.'); }
    const message = error.status === 409 ? 'The proposal or permission changed. Review the current state before continuing.' : error.message;
    notify(message, true);
  }
  finally { busy = false; publishing = false; await refresh(true); }
  return result;
}
async function begin(fresh = false) {
  const old = current();
  if (old && !fresh) { session().id = old.id; persist(); render(); return; }
  if (state?.items.some(item => item.operation?.state === 'OUTCOME_UNKNOWN') || session().uncertain) {
    notify('Resolve the uncertain publication before starting another copy. Open GitHub and inspect the saved outcome.', true); return;
  }
  if (fresh) saved[selected] = { events: [] };
  if (!session().key) session().key = crypto.randomUUID();
  persist();
  await run('Preparing the saved 8090 proposal…', async () => {
    const item = await post('/api/walkthrough/start', { scenario: selected, key: session().key });
    session().id = item.id; session().check = null; persist();
    eventLog('Saved 8090 draft prepared. Publication starts with permission revoked.');
    return item;
  });
}
async function permission(granted) {
  const revision = state.publication_permission.revision;
  await run(granted ? 'Granting publication permission…' : 'Revoking permission and invalidating earlier review…', async () => {
    const result = await post('/api/publication-permission', { granted, revision });
    eventLog(granted ? 'Publication permission granted. Exact human review is still required.' : 'Permission revoked. Earlier approvals cannot authorize a new release.');
    return result;
  });
}
async function evaluate() {
  const item = current(); if (!item) return;
  await run('Rust is checking permission and exact approval…', async () => {
    const result = await post(`/api/actions/${encodeURIComponent(item.id)}/evaluate`, binding(item));
    session().check = result; persist();
    eventLog(result.allowed ? 'Gate passed: current permission and exact approval match. Nothing sent yet.' : result.reason === 'permission_revoked' ? 'Gate blocked: permission revoked. Composio was not called.' : 'Gate blocked: current version needs human review. Composio was not called.');
    return result;
  });
}
function openReview() {
  reviewSnapshot = structuredClone(current()); if (!reviewSnapshot) return;
  $('#review-repo').textContent = reviewSnapshot.action.repository;
  $('#review-issue-title').textContent = reviewSnapshot.action.proposal.title;
  documentText($('#review-body'), reviewSnapshot.action.proposal.body);
  $('#review-binding').textContent = JSON.stringify({ version: reviewSnapshot.revision, account: reviewSnapshot.action.connected_account_id, tool: reviewSnapshot.action.tool, input_schema_sha256: reviewSnapshot.action.input_schema_sha256, context_digest: reviewSnapshot.context_digest }, null, 2);
  $('#review-dialog').showModal();
}
async function approve() {
  const item = reviewSnapshot; if (!item) return;
  const result = await run('Recording your approval of this exact version…', async () => {
    const value = await post(`/api/actions/${encodeURIComponent(item.id)}/approve`, binding(item));
    session().check = null;
    eventLog(`Human approved version ${item.revision}, bound to the current repository and policy.`); return value;
  });
  if (result) $('#review-dialog').close();
}
function openEdit() {
  editSnapshot = structuredClone(current()); if (!editSnapshot) return;
  $('#edit-issue-title').value = editSnapshot.action.proposal.title;
  $('#edit-body').value = editSnapshot.action.proposal.body; $('#edit-error').textContent = '';
  $('#edit-dialog').showModal();
}
async function saveEdit(event) {
  event.preventDefault(); if (!editSnapshot) return;
  const value = { revision: editSnapshot.revision, title: $('#edit-issue-title').value, body: $('#edit-body').value };
  const result = await run('Saving a new immutable version…', async () => {
    const item = await post(`/api/actions/${encodeURIComponent(editSnapshot.id)}/edit`, value);
    session().check = null; persist(); eventLog(`Version ${item.revision} saved. The earlier approval covers the previous version.`); return item;
  });
  if (result) $('#edit-dialog').close(); else $('#edit-error').textContent = 'The edit was not saved. Check the message on the walkthrough and try again with the current version.';
}
async function publish() {
  const item = current(); if (!item?.can_execute || session().uncertain) return;
  await run(state.mode === 'preview' ? 'Recording the approved preview…' : 'Rechecking the binding and submitting through Composio…', async () => {
    const result = await post(`/api/actions/${encodeURIComponent(item.id)}/execute`, binding(item));
    const op = result.operation;
    eventLog(op?.state === 'PUBLISHED' ? op.receipt?.simulated ? 'Approved preview recorded. No GitHub issue was created.' : 'Composio acknowledged the exact issue. GitHub receipt recorded.' : op?.state === 'OUTCOME_UNKNOWN' ? 'Outcome uncertain. No automatic resend is allowed.' : 'Publication stopped. Inspect the recorded operation.');
    return result;
  }, true);
}
function evidence() {
  const item = current(); if (!item) return;
  $('#evidence-json').textContent = JSON.stringify({ version: item.revision, publication_permission: state.publication_permission, action: item.action, context_digest: item.context_digest, decision: item.decision, reviews: item.review_history, receipt: item.operation?.receipt || null, operation_state: item.operation?.state || 'NOT_DISPATCHED', scope: item.evidence_scope }, null, 2);
  $('#evidence-dialog').showModal();
}
function phase(item) {
  if (!item) return { kind: 'intro', step: 0 };
  const history = item.review_history || [], reviewed = history.length > 0;
  const oldContext = history.some(review => !review.current);
  const priorCheck = session().check;
  const checked = priorCheck?.context_digest === item.context_digest && priorCheck.approved === item.approved && priorCheck.publication_allowed === item.publication_allowed ? priorCheck : null;
  if (item.operation || session().uncertain) return { kind: item.operation?.state === 'PUBLISHED' ? 'published' : 'uncertain', step: SCENARIOS[selected].steps.length - 1, checked };
  if (!item.publication_allowed) return { kind: reviewed ? checked && !checked.allowed ? 'restore' : 'revoked' : 'grant', step: reviewed && selected === 'revocation' ? checked ? 4 : 3 : 1, checked };
  if (!item.approved) {
    if (selected === 'revision' && item.revision > 1 && reviewed && !checked) return { kind: 'edited', step: 4, checked };
    return { kind: reviewed ? 'review_again' : 'review', step: selected === 'revocation' && reviewed ? 5 : selected === 'revision' && item.revision > 1 ? 4 : 2, checked };
  }
  if (selected === 'revocation' && !oldContext) return { kind: 'revoke', step: 3, checked };
  if (selected === 'revision' && item.revision === 1) return { kind: 'edit', step: 3, checked };
  return { kind: checked?.allowed ? 'publish' : 'check', step: selected === 'permission' ? checked?.allowed ? 4 : 3 : SCENARIOS[selected].steps.length - 1, checked };
}
function contentFor(p, item) {
  const again = selected === 'revision' ? 'The issue changed. Your earlier review did not.' : 'Permission is back. The old approval is not.';
  const views = {
    intro: { kicker: 'THE STARTING POINT', badge: 'READY TO EXPLORE', title: 'A proposal is ready.\nWhat should happen next?', copy: '8090 has prepared an issue. Bring its saved draft into this walkthrough with one click—no terminal, copying, or publishing.', primary: 'Start this walkthrough →', action: () => begin(), help: 'Loads the saved 8090 draft and starts with publication permission revoked.', quote: SCENARIOS[selected].quote, look: 'Click 8090 in the flow above to show the original conversation. Come back here to run the scenario.' },
    grant: { kicker: 'THE PERMISSION GATE', badge: 'PERMISSION REVOKED', tone: 'blocked', title: 'A ready proposal.\nNo permission to publish.', copy: 'The issue is prepared, but this gateway cannot send it to GitHub. Grant publication permission to continue.', primary: 'Grant publication permission →', action: () => permission(true), secondary: 'Check the blocked gate', extra: evaluate, help: 'Granting permission creates no issue and records no approval.', quote: 'The GitHub account is connected. That does not mean this gateway is allowed to publish. I control that permission right here.', look: 'Notice where the flow stops: TrustGraph. Composio and GitHub have not received a creation request.' },
    review: { kicker: 'THE HUMAN DECISION', badge: 'NEEDS HUMAN REVIEW', tone: 'review', title: 'Permission is granted.\nApproval is still missing.', copy: 'The destination is allowed. Now inspect the actual title, description, and repository before approving this version.', primary: 'Review this exact issue →', action: openReview, secondary: 'Check without approval', extra: evaluate, help: 'Review and publication are separate actions.', quote: 'Permission is one check. My review is another. I am approving this exact issue for this exact repository—not giving the agent a blank check.', look: 'Open the review panel. The content you see is the content the publisher will send.' },
    revoke: { kicker: 'THE CHANGE OF AUTHORITY', badge: 'APPROVED · NOT SENT', tone: 'ready', title: 'Approved a moment ago.\nRevoke permission now.', copy: 'The human review passed. Before anything is sent, take publication permission away and see what happens to that approval.', primary: 'Revoke publication permission', action: () => permission(false), danger: true, secondary: 'Inspect the approval', extra: evidence, help: 'Revocation takes effect before a new dispatch. No GitHub issue is created.', quote: 'I approved this issue, but I can still change who has authority to publish. A past approval cannot override a new restriction.', look: 'Watch the gate change from approved to blocked, while Composio stays idle.' },
    revoked: { kicker: 'THE GATE HAS CLOSED', badge: 'PERMISSION REVOKED', tone: 'blocked', title: 'The approval existed.\nThe authority no longer does.', copy: 'Permission changed after review. Check the gate to see why the previously approved issue cannot be sent.', primary: 'Check the revoked permission →', action: evaluate, help: 'This is a real authorization check. It never calls the publisher.', quote: 'Even though I reviewed the content, publication stops here. Human approval cannot bypass the current permission decision.', look: 'The result will identify the missing permission and show that Composio was not called.' },
    restore: { kicker: 'RESTORE, WITHOUT REUSING', badge: 'BLOCK CONFIRMED', tone: 'blocked', title: 'Blocked before Composio.\nNow restore permission.', copy: 'The server rejected the release check. Restore access and watch the previous approval stay stale.', primary: 'Restore publication permission →', action: () => permission(true), help: 'Restoring permission does not automatically approve the issue.', quote: 'Access can come back. The old authorization context does not. The reviewer must make a fresh decision.', look: 'After restoration, permission passes but review remains incomplete.' },
    review_again: { kicker: 'A FRESH HUMAN DECISION', badge: 'FRESH REVIEW REQUIRED', tone: 'review', title: again, copy: 'The current context is different from the one previously approved. Review and approve the exact version again.', primary: 'Review again →', action: openReview, secondary: 'Inspect what changed', extra: evidence, help: 'The earlier approval remains recorded; it cannot authorize this version and context.', quote: selected === 'revision' ? 'The original review stays in history. The edited issue gets a new version and must earn its own approval.' : 'Restoring access is not restoring consent. I review again because the publication context has changed.', look: 'Two independent checks are visible below. Permission passes; the current human review does not.' },
    edit: { kicker: 'THE EXACT VERSION MATTERS', badge: 'VERSION APPROVED', tone: 'ready', title: 'What happens if\nthe issue changes?', copy: 'Make an edit in the UI. TrustGraph records a new version and keeps the original approval attached to the old one.', primary: 'Edit the proposal →', action: openEdit, secondary: 'Inspect the approved version', extra: evidence, help: 'Editing changes the local proposal. It does not update or create a GitHub issue.', quote: 'I reviewed this version. Now I will change one requirement. The system must notice that my old decision no longer covers the work.', look: 'Use “Add an example requirement” in the editor, save it, and watch the version and review state change.' },
    edited: { kicker: 'THE STALE APPROVAL', badge: 'NEW VERSION · NOT APPROVED', tone: 'review', title: 'The edit is saved.\nThe old approval stays behind.', copy: 'Check the gate. Permission is still granted, but the new issue version has not been reviewed.', primary: 'Check the old approval →', action: evaluate, help: 'No provider request is made during this check.', quote: 'The destination did not change. Permission did not change. The content did—and that alone is enough to require a new review.', look: `Version ${item?.revision || 2} is shown on the issue below. The evidence panel keeps the previous review visible.` },
    check: { kicker: 'VERIFY BEFORE DISPATCH', badge: 'HUMAN REVIEW RECORDED', tone: 'ready', title: 'Permission and review.\nCheck them together.', copy: 'Ask the Rust gate to verify the current permission and the exact approved issue. Nothing is sent during this check.', primary: 'Verify the release gate →', action: evaluate, help: 'The final creation button appears after this check passes.', quote: 'Both decisions have to agree at the time of publication. The gate checks the current context, not a promise from earlier in the run.', look: 'The flow is ready at TrustGraph. Composio is still waiting for an explicit publication action.' },
    publish: { kicker: 'THE EXPLICIT RELEASE', badge: 'READY TO PUBLISH', tone: 'ready', title: state?.mode === 'preview' ? 'The gate passed.\nRecord the preview.' : 'The gate passed.\nCreate the real issue.', copy: state?.mode === 'preview' ? 'This server is in preview mode. It records a simulated receipt without calling GitHub.' : 'Your next click rechecks the binding and sends this exact issue through Composio. A successful response records its GitHub URL.', primary: state?.mode === 'preview' ? 'Record approved preview →' : 'Create GitHub issue ↗', action: publish, help: state?.mode === 'preview' ? 'Preview only. No external write.' : 'Live action: creates one issue in the displayed repository.', quote: 'Now the agent’s proposal becomes a real external action—with current permission, exact human review, and a receipt we can inspect.', look: 'Watch Composio process the request, then open the GitHub issue from the saved receipt.' },
    published: { kicker: 'THE OBSERVED OUTCOME', badge: item?.operation?.receipt?.simulated ? 'PREVIEW RECORDED' : 'PUBLISHED & RECORDED', tone: 'ready', title: item?.operation?.receipt?.simulated ? 'Preview complete.\nNo external write.' : 'Published.\nAnd accounted for.', copy: item?.operation?.receipt?.simulated ? 'The same gate recorded a simulated receipt. No GitHub issue was created in this run.' : 'Composio returned a matching GitHub issue. TrustGraph saved the outcome with the exact approved version and context.', primary: selected === 'revision' ? 'Replay this scenario ↺' : 'Explore the next scenario →', action: () => selected === 'revision' ? begin(true) : choose(selected === 'permission' ? 'revocation' : 'revision'), secondary: 'Inspect the receipt', extra: evidence, help: 'The receipt survives restart. Reopening it does not resend the issue.', quote: 'We can follow the whole decision: what was proposed, what was approved, whether permission still held, and what GitHub actually created.', look: 'Open the created issue. Then return here to explore another scenario or inspect the recorded evidence.' },
    uncertain: { kicker: 'STOP AND INSPECT', badge: 'OUTCOME NEEDS INSPECTION', tone: 'review', title: 'Do not guess.\nDo not send again.', copy: 'The operation did not produce a verified publication receipt. Inspect the saved state and GitHub before any further action.', primary: 'Refresh saved status →', action: () => refresh(true), secondary: 'Inspect the operation', extra: evidence, help: 'There is no automatic retry or resend.', quote: 'A missing response is not proof that nothing happened. The safe decision is to preserve the attempt and inspect its outcome.', look: 'Use the GitHub link in the sidebar to inspect the destination. Keep this recorded attempt intact.' }
  };
  return views[p.kind];
}
function renderCheck(id, passed, title, description, review = false) {
  const el = $(id); el.className = `check ${passed ? 'pass' : review ? 'review' : ''}`;
  const text = node('div'); text.append(node('strong', title), node('small', description));
  el.replaceChildren(node('span', passed ? '✓' : review ? '○' : '−', 'check-symbol'), text);
}
function renderFlow(item) {
  const published = item?.operation?.state === 'PUBLISHED';
  const blocked = item && !item.can_execute && !published;
  $('#flow-gate').className = `flow-node ${published || item?.can_execute ? 'ready' : blocked ? 'blocked' : ''}`;
  $('#gate-state').textContent = published ? 'Permission & review verified' : !item ? 'Your permission & review' : !item.publication_allowed ? 'Publication permission revoked' : item.approved ? 'Exact version approved' : 'Waiting for human review';
  $('#flow-provider').className = `flow-node ${publishing ? 'processing' : published ? 'published' : ''}`;
  $('#provider-state').textContent = publishing ? 'Checking & submitting…' : published ? item.operation.receipt?.simulated ? 'Not called · preview' : 'Acknowledgment received' : item?.operation ? 'Inspect saved outcome' : 'Not called · gate first';
  $('#flow-destination').className = `flow-node ${published && !item.operation.receipt?.simulated ? 'published' : ''}`;
  $('#destination-state').textContent = published ? item.operation.receipt?.simulated ? 'No issue · preview' : 'Issue created · open receipt ↗' : publishing ? 'Waiting for receipt…' : 'No new issue yet';
  $('#flow-destination').href = published && !item.operation.receipt?.simulated ? item.operation.receipt.provider_id : state?.story.github_url || $('#link-github').href;
}
function render() {
  const item = current(), p = phase(item), c = contentFor(p, item), scenario = SCENARIOS[selected];
  document.querySelectorAll('[data-scenario]').forEach(button => { const active = button.dataset.scenario === selected; button.classList.toggle('selected', active); if (active) button.setAttribute('aria-current', 'page'); else button.removeAttribute('aria-current'); });
  $('#chapter-label').textContent = `CHAPTER ${scenario.number}`; $('#chapter-title').textContent = scenario.title; $('#chapter-description').textContent = scenario.description;
  $('#mode').replaceChildren(node('i'), document.createTextNode(state?.mode === 'preview' ? 'PREVIEW · NO EXTERNAL WRITES' : 'LIVE WALKTHROUGH'));
  $('#stage-kicker').textContent = c.kicker; $('#stage-title').textContent = c.title; $('#stage-title').style.whiteSpace = 'pre-line'; $('#stage-copy').textContent = c.copy;
  $('#stage-badge').textContent = busy ? 'WORKING…' : c.badge; $('#stage-badge').className = `state-badge ${c.tone || ''}`;
  $('#primary').textContent = busy ? busyLabel : c.primary; $('#primary').classList.toggle('danger', !!c.danger); $('#primary').onclick = c.action;
  $('#secondary').hidden = !c.secondary || busy; $('#secondary').textContent = c.secondary || ''; $('#secondary').onclick = c.extra || null;
  $('#action-help').textContent = c.help; $('.stage').setAttribute('aria-busy', String(busy));
  $('#talk-quote').textContent = c.quote; $('#talk-look').textContent = c.look;
  $('#restart').hidden = !item || p.kind === 'uncertain';
  $('#steps').replaceChildren(...scenario.steps.map((label, index) => {
    const done = p.kind === 'published' || index < p.step; const el = node('li', undefined, done ? 'done' : index === p.step ? 'active' : '');
    if (index === p.step) el.setAttribute('aria-current', 'step'); el.append(node('span', done ? '✓' : String(index + 1)), node('span', label)); return el;
  }));
  const verdict = $('#verdict'), check = p.checked;
  verdict.hidden = !check || p.kind === 'published' || p.kind === 'intro';
  if (check) { verdict.classList.toggle('ready', check.allowed); verdict.textContent = check.allowed ? '✓ Release checks passed. Permission and exact approval match. Nothing has been sent yet.' : check.reason === 'permission_revoked' ? '− Blocked before Composio. Publication permission is revoked. No issue creation was attempted.' : '− Blocked before Composio. The current version and context need a valid human approval.'; }
  let receiptLink = $('#receipt-link'); if (receiptLink) receiptLink.remove();
  if (p.kind === 'published' && !item.operation.receipt?.simulated) { receiptLink = node('a', 'Open the created GitHub issue ↗', 'receipt-link'); receiptLink.id = 'receipt-link'; receiptLink.href = item.operation.receipt.provider_id; receiptLink.target = '_blank'; receiptLink.rel = 'noreferrer'; $('.stage-actions').before(receiptLink); }
  $('#workspace').hidden = !item;
  if (item) {
    $('#version').textContent = `VERSION ${item.revision}`; $('#proposal-title').textContent = item.action.proposal.title; $('#repository').textContent = item.action.repository;
    const fingerprint = `${item.id}:${item.revision}`;
    if ($('#proposal-body').dataset.version !== fingerprint) { documentText($('#proposal-body'), item.action.proposal.body); $('#proposal-body').dataset.version = fingerprint; }
    $('#proposal-origin').textContent = item.revision > 1 ? 'Edited in TrustGraph · earlier version retained' : 'Based on the saved native 8090 draft';
    renderCheck('#permission-check', item.publication_allowed, item.publication_allowed ? 'Permission granted' : 'Permission revoked', 'Gateway authority to create issues');
    renderCheck('#review-check', item.approved, item.approved ? 'Exact version approved' : item.review_history?.length ? 'Fresh human review required' : 'Human review required', `Version ${item.revision} · current destination & policy`, true);
    const events = session().events.length ? session().events : [{ text: 'Resumed the saved proposal and its current authorization state.', at: new Date().toISOString() }];
    $('#activity-list').replaceChildren(...events.slice(0, 4).map(entry => { const li = node('li', entry.text); const time = node('time', new Date(entry.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })); time.dateTime = entry.at; li.append(time); return li; }));
  }
  if (state?.story) {
    $('#link-8090').href = $('#flow-source').href = state.story.source_url;
    $('#link-composio').href = $('#flow-provider').href = state.story.composio_url; $('#link-github').href = state.story.github_url;
    const receipt = state.story.verified_receipt;
    if (receipt) { $('#proof-link').href = receipt.provider_id; $('#proof-link strong').textContent = 'Real integration. Real result.'; $('#proof-text').textContent = 'Live receipt verified · GitHub issue ↗'; }
    else { $('#proof-link').href = state.story.github_url; $('#proof-link strong').textContent = 'Explore the destination'; $('#proof-text').textContent = 'Configured GitHub repository ↗'; }
  }
  renderFlow(item);
  document.querySelectorAll('button').forEach(button => { button.disabled = busy && !button.classList.contains('close'); });
  $('#primary').disabled = busy || !authenticated;
}
function choose(scenario) { if (busy) return; selected = scenario; persist(); notify(); render(); $('#chapter-title').scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'instant' : 'smooth', block: 'start' }); }
async function start() {
  document.querySelectorAll('[data-scenario]').forEach(button => button.addEventListener('click', () => choose(button.dataset.scenario)));
  document.querySelectorAll('[data-close]').forEach(button => button.addEventListener('click', () => $(`#${button.dataset.close}`).close()));
  $('#approve-confirm').addEventListener('click', approve); $('#edit-form').addEventListener('submit', saveEdit); $('#evidence').addEventListener('click', evidence); $('#restart').addEventListener('click', () => begin(true));
  $('#example-edit').addEventListener('click', () => { const addition = '\n\n## Additional acceptance criterion\nShow the reviewed version and repository in the publication receipt.'; if (!$('#edit-body').value.includes(addition.trim())) $('#edit-body').value += addition; $('#edit-body').focus(); });
  let notesVisible = saved.notes !== false;
  const updateNotes = () => { document.body.classList.toggle('notes-hidden', !notesVisible); $('#notes-toggle').setAttribute('aria-pressed', String(notesVisible)); };
  $('#notes-toggle').addEventListener('click', () => { notesVisible = !notesVisible; saved.notes = notesVisible; persist(); updateNotes(); }); updateNotes(); render();
  const token = location.hash.slice(1); history.replaceState(null, '', location.pathname);
  if (/^[a-f0-9]{64}$/i.test(token)) {
    try { await api('/api/session', { method: 'POST', headers: { Authorization: `Bearer ${token}` } }); }
    catch { notify('The operator sign-in did not complete. Reopen the private launch file in this browser.', true); }
  }
  await refresh(); setInterval(() => refresh(), 4000);
}
start();
