const main = document.querySelector('#main');
const hostedDemo = document.querySelector('meta[name="trustgraph-mode"]')?.content === 'hosted';
const historyKey = 'trustgraph:demo:v1';
let demoHistory = [];
if (hostedDemo) {
  try {
    const saved = JSON.parse(sessionStorage.getItem(historyKey) || '[]');
    if (Array.isArray(saved) && saved.length <= 64) demoHistory = saved;
  } catch { /* Storage is optional; this tab can still run the demo. */ }
}
const clone = value => JSON.parse(JSON.stringify(value));
const escape = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const chapters = [
  ['company', 'The company'], ['policy', 'The policy'], ['simulator', 'Test the boundary'],
  ['enforce', 'Enforce the policy'], ['revision', 'When work changes'],
];
const departments = {hr:'HR',finance:'Finance',engineering:'Engineering'};
const icons = {
  hr:'<circle cx="9" cy="7" r="3"/><path d="M3 19v-2a6 6 0 0 1 12 0v2M17 4a3 3 0 0 1 0 6M18 14a5 5 0 0 1 3 5"/>',
  finance:'<rect x="3" y="5" width="18" height="15" rx="2"/><path d="M3 10h18M7 15h4M16 14v3M14.5 15.5h3"/>',
  engineering:'<path d="m8 6-6 6 6 6M16 6l6 6-6 6M14 3l-4 18"/>',
};
let state, candidate, comparison, comparisonSignature, previousFingerprint, noticeTimer, busy = false, editingPolicy = false;
let scenario = {inputs:['budget','compensation','directory'],destination:'engineering',approval:'missing',approval_age_minutes:0,complete_lineage:true};
let selectedPreset = 'private';
const step = () => chapters.some(([id]) => id === location.hash.slice(1)) ? location.hash.slice(1) : 'company';
const source = id => state.sources.find(s => s.id === id);
const sourceTitle = id => source(id)?.title || (id === 'task-prompt' ? 'Work order' : id);
const person = id => state.people.find(p => p.id === id);
const pill = (text, tone='') => `<span class="pill ${tone}">${escape(text)}</span>`;
const primary = (text, attributes='') => `<button class="primary" ${attributes}>${escape(text)}<span class="arrow" aria-hidden="true">↗</span></button>`;
const link = (text, id) => `<a class="primary" href="#${id}">${escape(text)}<span class="arrow" aria-hidden="true">→</span></a>`;
const pretty = value => escape(JSON.stringify(value,null,2));
const title = (chapter, heading, description) => `<div class="eyebrow"><span></span>${escape(chapter)}</div><div class="heading"><div><h1>${heading}</h1><p>${escape(description)}</p></div><span class="chapter-index">NORTHSTAR / TG-104</span></div>`;
const footer = (takeaway, next, nextLabel) => `<div class="chapter-footer"><p class="takeaway">${takeaway}</p>${link(nextLabel,next)}</div>`;
const checkRow = (title,value,tone='') => `<div class="check-row"><dt>${escape(title)}</dt><dd class="${tone}">${escape(value)}</dd></div>`;
const evidence = (label,value) => `<details class="evidence"><summary>${escape(label)}</summary><pre>${pretty(value)}</pre></details>`;
// Read the body as text first: proxies and extractor rejections may not send JSON.
async function readResponse(response) {
  let text = '';
  try { text = await response.text(); } catch { /* Treat an unreadable body as empty. */ }
  let value = null;
  try { value = text ? JSON.parse(text) : null; } catch { value = null; }
  if (!response.ok) {
    const detail = value && typeof value.error === 'string' ? value.error
      : `The server returned ${response.status}${text ? `: ${text.slice(0, 160)}` : ''}.`;
    const error = Error(detail); error.status = response.status; throw error;
  }
  if (!value || typeof value !== 'object') throw Error('The server returned an unreadable response. Please retry.');
  return value;
}
async function api(path='', body) {
  if (hostedDemo) {
    const history = path === '/reset' ? [] : demoHistory;
    const request = {history};
    if (path === '/compare') request.comparison = body;
    else if (body !== undefined && path !== '/reset') request.command = body;
    const response = await fetch('/api/demo', {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(request)});
    const value = await readResponse(response);
    demoHistory = request.command ? [...history, request.command] : history;
    try { sessionStorage.setItem(historyKey, JSON.stringify(demoHistory)); } catch { /* Keep progress in memory. */ }
    return value;
  }
  const response = await fetch('/api/story'+path, {method:body===undefined?'GET':'POST',headers:body===undefined?{}:{'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body)});
  return readResponse(response);
}
// A saved history from an older deployment can be rejected (4xx). Start over once.
async function loadInitialState() {
  try { return {value: await api(), restarted: false}; }
  catch (error) {
    if (!hostedDemo || !demoHistory.length || !(error.status >= 400 && error.status < 500)) throw error;
    demoHistory = [];
    try { sessionStorage.removeItem(historyKey); } catch { /* Storage is optional. */ }
    return {value: await api(), restarted: true};
  }
}
const maxApprovalAge = 10080;
function approvalAge(text) {
  const value = Number(text);
  return String(text).trim() !== '' && Number.isInteger(value) && value >= 0 && value <= maxApprovalAge ? value : null;
}
function notify(message) {
  const notice = document.querySelector('#notice');
  clearTimeout(noticeTimer); notice.textContent = message; notice.hidden = false;
  noticeTimer = setTimeout(() => {notice.hidden=true;},6500);
}
async function perform(task) {
  if (busy) return;
  busy=true;
  const focused = document.activeElement?.id;
  document.querySelectorAll('button').forEach(b=>b.disabled=true);
  try { await task(); } catch (error) { notify(error.message); }
  finally {
    busy=false; render();document.querySelector('#reset').disabled=false;
    const target=focused&&document.getElementById(focused);
    (target||main).focus({preventScroll:true});
  }
}
async function command(body) { state=await api('',body); }
function companyPage() {
  const cards = [
    ['hr','People & compensation','Nina · People operations',['Employee compensation','Shared planning data']],
    ['finance','Budgets & forecasts','Leo · Financial planning',['Department budgets','Shared with HR for planning']],
    ['engineering','Build & deliver','Sam · Engineering',['Headcount dashboard','Engineering repository']],
  ];
  return title('01 / MEET THE COMPANY','One company.<br>Different boundaries.','Northstar has three departments and one shared agent. Information can be useful across the company without being available to everyone.')+
    `<div class="section-head"><h2>Three departments. Different responsibilities.</h2><small>ONE COMPANY</small></div><div class="department-grid">${cards.map(([id,description,owner,assets],i)=>`<section class="department-card"><div class="department-top"><span class="department-icon ${id}" aria-hidden="true"><svg viewBox="0 0 24 24">${icons[id]}</svg></span><span class="department-count">0${i+1} / DEPARTMENT</span></div><h2>${departments[id]}</h2><p class="owner">${escape(description)}</p><div class="department-assets"><small>${escape(owner)}</small>${assets.map(a=>`<p><span class="asset-icon" aria-hidden="true">↳</span>${escape(a)}</p>`).join('')}</div></section>`).join('')}</div>
    <div class="agent-bridge"><span class="agent-symbol" aria-hidden="true">[a]</span><div><strong>One agent can work across all three departments.</strong><p>Its access helps it do the work. It does not give every department access to everything it uses.</p></div><span class="agent-tag">SHARED AGENT ≠ SHARED PERMISSIONS</span></div>
    <div class="work-intro"><section><span class="section-label">THE WORK WE’LL FOLLOW / TG-104</span><h2>Build a headcount<br>planning dashboard.</h2><p>Engineering needs a dashboard showing department headcounts and budget bands. The agent can also read HR’s individual compensation data.</p><div class="work-destination">INTENDED DESTINATION <span>↗ Engineering repository</span></div></section><section class="question-panel"><span class="section-label">THE GOVERNANCE QUESTION</span><blockquote>Can Engineering access this dashboard, given the data used to create it?</blockquote><p>We’ll define the boundary, test it, then follow this same work item through release and revision.</p></section></div>`+
    footer('<strong>Access to an input is not permission to share its output.</strong> First, let’s make the company’s rules explicit.','policy','Explore the policy');
}
function permissionMatrix(policy,editable) {
  return `<div class="matrix-wrap"><table class="matrix"><caption class="sr-only">Departments allowed to access work created from each data source</caption><thead><tr><th scope="col">SOURCE INFORMATION</th>${Object.entries(departments).map(([id,name])=>`<th scope="col" class="${id}">${name.toUpperCase()}</th>`).join('')}</tr></thead><tbody>${state.sources.map(s=>`<tr><td><strong>${escape(s.title)}</strong><small>${s.id==='approved-planning'?'HR · Headcounts and budget bands. No salaries.':'Owner: '+escape(s.owner)}</small></td>${Object.entries(departments).map(([id,name])=>`<td>${editable?`<label><input type="checkbox" data-permission="${s.id}" value="${id}" aria-label="${escape(s.title)}: ${name} access" ${policy.source_readers[s.id].includes(id)?'checked':''}></label>`:`<span class="${policy.source_readers[s.id].includes(id)?'permission-yes':'permission-no'}" aria-label="${name}: ${policy.source_readers[s.id].includes(id)?'permitted':'restricted'}">${policy.source_readers[s.id].includes(id)?'✓':'—'}</span>`}</td>`).join('')}</tr>`).join('')}</tbody></table></div>`;
}
function policyPage() {
  const p = editingPolicy ? candidate : state.policy;
  return title('02 / DEFINE THE BOUNDARY','The policy is the boundary.','Define who can access work derived from each source, who can approve it, and how long that approval lasts.')+
    `<div class="policy-layout"><div><section class="panel"><div class="panel-head"><h2>${editingPolicy?'Proposed policy':'Department boundaries'}</h2>${editingPolicy?pill('CHANGES NOT APPLIED','neutral'):pill('CURRENT · V'+state.policy_revision,'good')}</div>${editingPolicy?'<div class="policy-edit-banner">These are proposed changes. Test them first, then choose Apply policy changes to use them.</div>':''}${permissionMatrix(p,editingPolicy)}<p class="matrix-note">✓ Can access work created from this data &nbsp; — Restricted<br>Source access is explicit. Maya, the governance lead, can inspect all sources. The shared agent can read them but cannot approve a release.</p></section>
    <section class="panel margin-top"><div class="panel-body">${editingPolicy?`<label for="policy-name">Policy name</label><input class="policy-name" id="policy-name" maxlength="80" value="${escape(p.name)}">`:''}<div class="policy-review"><div class="compact-field"><label>WHO CAN APPROVE</label>${editingPolicy?`<div class="reviewer-options">${state.people.map(p=>`<label><input type="checkbox" data-reviewer="${p.id}" ${candidate.approvers.includes(p.id)?'checked':''}>${escape(p.name)}</label>`).join('')}</div>`:`<div class="value">${p.approvers.map(id=>escape(person(id).name)).join(' · ')}</div><small>A reviewer must also be able to read the work.</small>`}</div><div class="compact-field"><label for="policy-lifetime">APPROVAL LIFETIME</label>${editingPolicy?`<select id="policy-lifetime">${[5,15,30,60,120].map(n=>`<option value="${n}" ${p.approval_minutes===n?'selected':''}>${n} minutes</option>`).join('')}</select>`:`<div class="value">${p.approval_minutes} minutes</div><small>Bound to the exact version and destination.</small>`}</div></div></div></section>
    <div class="section-head margin-top"><button class="text-button" id="toggle-policy">${editingPolicy?'← View current policy':'Edit proposed policy ↗'}</button>${editingPolicy?'<button class="text-button" id="restrict-preset">Try: restrict planning data to HR + Finance</button>':''}</div>${evidence('Inspect the structured policy',editingPolicy?candidate:state.policy_spec)}</div>
    <aside class="panel rules"><h2>Always enforced</h2><div class="rule"><span>01</span><div><h3>Restrictions follow the work.</h3><p>An output inherits the restrictions of every recorded input, including earlier drafts.</p></div></div><div class="rule"><span>02</span><div><h3>Approval has a scope.</h3><p>Approval covers this version, the department with access, and the policy. Changed work needs a new review.</p></div></div><div class="rule"><span>03</span><div><h3>Unknown inputs stop release.</h3><p>Missing input records block publishing. A reviewer cannot override source permissions.</p></div></div></aside></div>`+
    footer('A policy defines the rules. <strong>The simulator lets us test their consequences before applying a change.</strong>','simulator',editingPolicy?'Test proposed changes':'Test the policy');
}
const statusText = result => result.status==='ready'?'Ready to release':result.status==='needs_approval'?'Needs approval':'Blocked';
function decisionCard(result,label) {
  return `<section class="decision-card ${result.status}"><div class="card-kicker">${escape(label)}</div><h3>${result.status==='ready'?'Would allow':statusText(result)}</h3><p>${escape(result.explanation)}</p><dl class="checks">${checkRow('Department access',result.information_permitted?'Allowed':'Blocked',result.information_permitted?'good':'bad')}${checkRow('Approval for this version',result.approval_valid?'Approved for this test':!result.information_permitted?'Cannot grant access':result.code==='STALE_APPROVAL'?'Out of date':result.code==='APPROVAL_EXPIRED'?'Expired':'Required',result.approval_valid?'good':'')}${checkRow('Department',departments[scenario.destination])}</dl>${result.conflicting_sources.length?`<p class="margin-top">Data blocking access:<br><strong>${result.conflicting_sources.map(id=>escape(sourceTitle(id))).join(', ')}</strong></p>`:''}</section>`;
}
function signature() { return JSON.stringify({policy:candidate,scenario}); }
function simulationResult() {
  if (!comparison || comparisonSignature!==signature()) return `<div class="sim-placeholder"><span class="placeholder-symbol" aria-hidden="true">[ ? ]</span><h2>What would this policy allow?</h2><p>Choose the data, department and approval status. Compare the current policy with your proposed policy.</p>${primary('Run comparison','id="compare-empty"')}</div><p class="simulation-note">This tests access to the dashboard. Nothing is generated or published, and the current policy stays in use.</p>`;
  const changed = JSON.stringify(candidate)!==JSON.stringify(state.policy);
  return `<div class="lab-top">${pill('COMPARISON COMPLETE','good')}${pill('NOTHING PUBLISHED')}</div><div class="comparison">${decisionCard(comparison.active,'CURRENT POLICY · V'+comparison.policy_revision)}${decisionCard(comparison.candidate,changed?'PROPOSED POLICY':'PROPOSED POLICY · NO CHANGES YET')}</div><p class="comparison-caption">${comparison.active.status===comparison.candidate.status?'Both policies give this dashboard the same result for the selected department.':'Your proposed policy changes the result for this dashboard.'}</p><p class="fingerprint">RESULT ID &nbsp; ${escape(comparison.fingerprint.slice(0,24))}…<br>Repeat the test with the same data, rules and time to get the same result.</p>
    <div class="candidate-summary"><div><strong>${escape(candidate.name)}</strong><p>${changed?'Proposed changes tested. The current policy is still in use.':'No policy changes yet. Both columns use the current rules.'}</p></div>${changed?'<button class="secondary" id="activate-candidate">Apply policy changes</button>':'<a class="text-button" href="#policy" id="edit-from-sim">Edit proposed policy →</a>'}</div>${evidence('View test details',comparison)}`;
}
function simulatorPage() {
  const presets=[['private','Salary + budget data → Engineering'],['hr','Salary + budget data → HR'],['approved','Shared planning data → Engineering'],['changed','Dashboard edited after approval']];
  return title('03 / TEST BEFORE YOU ACT','Test a policy change.<br>See who can access the dashboard.','Choose the data and department. Compare the current policy with your proposed changes before applying them.')+
    `<div class="sim-presets" aria-label="Scenario presets">${presets.map(([id,label])=>`<button data-preset="${id}" class="${selectedPreset===id?'selected':''}" aria-pressed="${selectedPreset===id}">${escape(label)}</button>`).join('')}</div>
    <div class="sim-layout"><section class="panel"><div class="panel-head"><h2>Proposed work · TG-104</h2>${pill('WHAT IF')}</div><fieldset class="sim-inputs"><legend class="sr-only">Inputs used by the agent</legend>${state.sources.map(s=>`<label class="input-choice"><input type="checkbox" data-input="${s.id}" ${scenario.inputs.includes(s.id)?'checked':''}><span><strong>${escape(s.title)}</strong><small>${escape(s.description)}</small><span class="source-scope">${state.policy.source_readers[s.id].map(id=>departments[id]).join(' + ')}</span></span></label>`).join('')}</fieldset><div class="sim-controls"><div><label for="sim-destination">Which department needs access?</label><select id="sim-destination">${Object.entries(departments).map(([id,name])=>`<option value="${id}" ${scenario.destination===id?'selected':''}>${name}</option>`).join('')}</select></div><div><label for="sim-approval">Approval status for this test</label><select id="sim-approval">${[['missing','No approval yet'],['valid','This version is approved'],['changed','Changed after approval'],['expired','Expired approval']].map(([id,name])=>`<option value="${id}" ${scenario.approval===id?'selected':''}>${name}</option>`).join('')}</select></div><label class="inline-check"><input type="checkbox" id="sim-lineage" ${scenario.complete_lineage?'checked':''}>All inputs are recorded</label><div><label for="sim-age">Minutes since approval</label><input type="number" id="sim-age" min="0" max="10080" step="1" inputmode="numeric" value="${escape(scenario.approval_age_minutes)}"></div>${primary('Run comparison','id="compare"')}</div></section>
    <section id="comparison-results" aria-label="Policy comparison" aria-live="polite">${simulationResult()}</section></div>`+
    footer('<strong>Simulation and release use the same verifier.</strong> The simulator evaluates explicit assumptions; the release gate checks recorded work and approval.','enforce','See the policy enforced');
}
function history() {
  if (!state.versions.length) return '';
  return `<section class="history"><div class="section-head"><h2>One work item. Every version.</h2><small>TG-104</small></div>${state.versions.map(v=>`<div class="history-row"><span class="version-number">V${v.version}</span><div><strong>${escape(v.event)}</strong><small>${v.fresh_context?'Fresh captured context':'Previous version retained as an input'} · ${escape(v.artifact)}</small><div class="history-hash">SHA-256 ${escape(v.hash.slice(0,20))}…</div></div>${v.operation?pill('RELEASE RECORDED','good'):pill(v.assessment.code==='STALE_APPROVAL'?'REVIEW REQUIRED':statusText(v.assessment),v.assessment.status==='blocked'?'bad':v.assessment.status==='ready'?'good':'')}</div>`).join('')}</section>`;
}
function flow(current) {
  const ids = current.inputs.filter(id=>id!=='task-prompt');
  const result = current.assessment;
  const sourceNodes = ids.map(id=>`<span class="flow-source ${result.conflicting_sources.includes(id)?'restricted':''}">${escape(sourceTitle(id))}</span>`).join('');
  return `<div class="flow-diagram"><div class="flow-label">CAPTURED INPUTS → DERIVED WORK → DESTINATION</div><div class="flow-sources">${sourceNodes}</div><div class="flow-down" aria-hidden="true">↓</div><div class="flow-output"><strong>Headcount dashboard · V${current.version}</strong>${pill(current.fresh_context?'FRESH CONTEXT':'EDITED VERSION')}</div><div class="flow-target"><span class="${result.information_permitted?'':'barrier'}" aria-hidden="true">${result.information_permitted?'↳':'✕'}</span><span>Engineering repository</span><span class="${result.information_permitted?'engineering':'hr'}">${result.information_permitted?'Information permitted':'Source boundary blocks delivery'}</span></div></div>`;
}
function gate(current,revision=false) {
  const a=current.assessment, released=!!current.operation, reviewers=current.eligible_reviewers;
  const heading = released?'Release recorded.':a.code==='STALE_APPROVAL'?'New bytes.<br>New review.':!a.information_permitted?'The boundary<br>holds.':a.approval_valid?'This version<br>is approved.':'Permitted.<br>Not yet approved.';
  const actions = !a.information_permitted?primary('Regenerate with shared data','id="repair"'):released?`<div class="margin-top">${pill('RELEASE RECORDED','good')}</div>`:a.approval_valid?primary(revision?'Release revised version':'Release this version','id="release"'):`<label for="reviewer">Review as</label><select id="reviewer">${reviewers.map(id=>`<option value="${id}" ${id==='maya'?'selected':''}>${escape(person(id).name)} · ${escape(person(id).department)}</option>`).join('')||'<option>No eligible reviewer</option>'}</select>${primary(a.code==='STALE_APPROVAL'?'Approve the revised version':'Approve this exact version',`id="approve" ${reviewers.length?'':'disabled'}`)}`;
  return `<aside class="panel gate-panel"><span class="gate-kicker">CURRENT RELEASE DECISION</span><h2 class="${!a.information_permitted||a.code==='STALE_APPROVAL'?'bad':a.approval_valid?'good':''}">${heading}</h2><p>${escape(released?'A release record is saved for this exact version and destination. Earlier release records stay in history.':a.explanation)}</p><dl class="checks">${checkRow('Source permissions',a.information_permitted?'Permitted':'Blocked',a.information_permitted?'good':'bad')}${checkRow('Approval for this version',a.approval_valid?'Matches':a.code==='STALE_APPROVAL'?'Out of date':'Required',a.approval_valid?'good':a.code==='STALE_APPROVAL'?'bad':'')}${checkRow('Department','Engineering')}${checkRow('Policy','V'+state.policy_revision)}</dl>${actions}<p class="action-note">${!a.information_permitted?'Start a fresh run with the organization directory and HR’s shared planning data. The private compensation data and restricted draft are excluded.':'Approval covers this exact version and destination. Release is recorded locally; no repository is changed.'}</p></aside>`;
}
function enforcePage() {
  const c=state.current;
  let body;
  if (!c) {
    body=`<div class="work-layout"><section class="panel work-item"><span class="section-label">WORK ORDER / TG-104</span><h2>Build a headcount<br>planning dashboard.</h2><p>The agent’s initial context includes the organization directory, HR compensation and department budgets. It proposes a change for Engineering.</p><div class="flow-diagram"><div class="flow-label">INITIAL CONTEXT</div><div class="flow-sources"><span class="flow-source">Organization directory</span><span class="flow-source restricted">Employee compensation</span><span class="flow-source">Department budgets</span></div><div class="flow-down" aria-hidden="true">↓</div><div class="flow-output">Proposed dashboard<span class="engineering">→ Engineering</span></div></div><p>The new version records the information used to create it. The policy then determines whether Engineering can access it.</p></section><aside class="panel gate-panel"><span class="gate-kicker">FROM POLICY TO ENFORCEMENT</span><h2>Let’s follow<br>the actual work.</h2><p>Create the first version, then check whether Engineering may access it under the active policy.</p>${primary('Generate the first version','id="generate"')}<p class="action-note">The policy applies to this work item and every version that follows.</p></aside></div>`;
  } else {
    body=`<div class="lab-top">${pill('WORK ORDER TG-104')}${pill('VERSION '+c.version)}${pill('POLICY V'+state.policy_revision,'good')}</div><div class="work-layout"><section class="panel work-item"><span class="section-label">THE SAME WORK ITEM / TG-104</span><h2>Headcount planning dashboard</h2><p>${c.fresh_context&&c.inputs.includes('approved-planning')?'HR shares department headcounts and budget bands with Engineering, while individual salaries remain private. This new version uses that shared data and the organization directory.':'The input manifest is recorded with this exact artifact. The gate evaluates its full dependency history.'}</p>${flow(c)}<div class="code-head"><span>dashboard/planning.ts · VERSION ${c.version}</span><span>EXACT RECORDED CONTENT</span></div><pre class="code">${escape(c.artifact.content)}</pre><p class="fingerprint margin-top">SHA-256 &nbsp;${escape(c.artifact.hash)}</p>${evidence('Inspect captured inputs and the Rust decision',{work_id:state.work_id,artifact:c.artifact.id,inputs:c.inputs,decision:c.assessment.decision})}</section>${gate(c)}</div>${history()}`;
  }
  return title('04 / ENFORCE THE POLICY','Useful work still needs<br>permission to leave.','Follow TG-104 from its first restricted version through regeneration, review and release.')+body+
    footer('<strong>Source permission and human approval are separate gates.</strong> A passing source check still needs an exact human approval.','revision','See what happens after an edit');
}
function revisionPage() {
  const c=state.current, previous=state.versions.at(-2);
  let body;
  if (!c || !c.assessment.information_permitted || (c.fresh_context&&!c.assessment.approval_valid)) {
    body=`<section class="panel empty-work"><div class="empty-symbol" aria-hidden="true">[ v+1 ]</div><h2>First, approve a permitted version.</h2><p>This chapter uses the same artifact you created in the previous step. Regenerate using shared planning data, then approve that exact version to see what happens when it changes.</p>${link('Return to the work item','enforce')}</section>`;
  } else if (c.fresh_context) {
    body=`<div class="work-layout"><section class="panel work-item"><span class="section-label">TG-104 / APPROVED VERSION</span><h2>Version ${c.version} is approved.<br>Now the request changes.</h2><p>Engineering asks to add a contractor toggle to the dashboard. It’s a small change, using the same permitted inputs. Does the existing approval still cover it?</p><div class="revision-visual"><div class="version-card"><small>REVIEWED ARTIFACT</small><h3>V${c.version}</h3>${pill(c.operation?'RELEASED':'APPROVED','good')}<p class="margin-top">${escape(c.artifact.hash.slice(0,18))}…</p></div><span class="version-arrow" aria-hidden="true">→</span><div class="version-card current"><small>PROPOSED CHANGE</small><h3>V${c.version+1}</h3><p>Add a contractor toggle.<br>Different content. Same destination.</p></div></div>${primary('Revise the approved artifact','id="revise"')}<p class="simulation-note">The change creates a new version. The original artifact and its approval remain in history.</p></section>${gate(c,true)}</div>${history()}`;
  } else {
    body=`<div class="work-layout"><section class="panel work-item"><span class="section-label">SAME WORK ORDER / NEW BYTES</span><h2>The information is permitted.<br>The old approval isn’t.</h2><p>A new artifact records the revised content and retains the previous version as an input. Nothing rewrites the artifact that was already reviewed.</p><div class="revision-visual"><div class="version-card"><small>PREVIOUS VERSION</small><h3>V${previous?.version||c.version-1}</h3>${pill(previous?.operation?'RELEASED · HISTORICAL':'REVIEWED · HISTORICAL','good')}<p class="margin-top">${escape(previous?.hash.slice(0,18)||'')}…</p></div><span class="version-arrow" aria-hidden="true">→</span><div class="version-card current"><small>CURRENT VERSION</small><h3>V${c.version}</h3>${pill(c.assessment.approval_valid?'NEW APPROVAL MATCHES':'NEW REVIEW REQUIRED',c.assessment.approval_valid?'good':'bad')}<p class="margin-top">${escape(c.artifact.hash.slice(0,18))}…</p></div></div><div class="binding"><div><small>OLD ARTIFACT HASH</small><code>${escape(previous?.hash.slice(0,26)||'')}…</code></div><div><small>CURRENT ARTIFACT HASH</small><code>${escape(c.artifact.hash.slice(0,26))}…</code></div></div><pre class="code">${escape(c.artifact.content)}</pre><p class="simulation-note">An earlier release remains historical fact. Its approval grants no permission to publish these changed bytes.</p></section>${gate(c,true)}</div>${history()}`;
  }
  return title('05 / WHEN WORK CHANGES','Approval belongs to a version.<br>Not to a work order.','An edit does not revoke the reviewer’s role or erase an earlier release. It creates new work that must pass the gate again.')+body+
    `<details class="evidence"><summary>Follow the complete work-item history</summary><ol class="live-log">${state.events.map((event,i)=>`<li><span>${String(i+1).padStart(2,'0')}</span>${escape(event)}</li>`).join('')}</ol></details>`+
    footer('<strong>One company. Explicit boundaries. Every version accountable.</strong> Return to the simulator to test another policy against the same rules.','simulator','Explore another policy');
}
function invalidateComparison() {
  comparison=undefined; selectedPreset='';
  document.querySelectorAll('[data-preset]').forEach(button=>{button.classList.remove('selected');button.setAttribute('aria-pressed','false');});
  const host=document.querySelector('#comparison-results');
  if (host) {host.innerHTML=simulationResult();bindResults();}
}
async function compare() {
  if (approvalAge(scenario.approval_age_minutes) === null) throw Error(`Minutes since approval must be a whole number from 0 to ${maxApprovalAge}.`);
  const sentSignature=signature();
  const result=await api('/compare',{policy:candidate,scenario});
  if (sentSignature!==signature()) {comparison=undefined;notify('Inputs changed during evaluation. Run the comparison again.');return;}
  const repeated=previousFingerprint===result.fingerprint;
  comparison=result; comparisonSignature=sentSignature; previousFingerprint=result.fingerprint;
  notify(repeated?'Same decision. Same fingerprint. The comparison is reproducible.':'Comparison complete. The current policy and dashboard are unchanged.');
}
function bindResults() {
  document.querySelector('#compare-empty')?.addEventListener('click',()=>perform(compare));
  document.querySelector('#activate-candidate')?.addEventListener('click',()=>perform(async()=>{
    if (!comparison || comparisonSignature!==signature()) throw Error('Run this comparison again before applying the policy changes.');
    await command({action:'activate',policy:candidate,expected_revision:comparison.policy_revision});
    candidate=clone(state.policy); comparison=undefined; editingPolicy=false;
    notify('Policy changes applied. Publishing now uses policy V'+state.policy_revision+'.');
  }));
  document.querySelector('#edit-from-sim')?.addEventListener('click',()=>{editingPolicy=true;});
}
function bind() {
  document.querySelector('#toggle-policy')?.addEventListener('click',()=>{editingPolicy=!editingPolicy;if(editingPolicy)candidate=clone(state.policy);comparison=undefined;render();});
  document.querySelector('#restrict-preset')?.addEventListener('click',()=>{candidate.source_readers['approved-planning']=['finance','hr'];comparison=undefined;render();notify('Proposed change: only HR and Finance can access shared planning data. Test Shared planning data → Engineering.');});
  document.querySelectorAll('[data-permission]').forEach(input=>input.addEventListener('change',()=>{
    const members=new Set(candidate.source_readers[input.dataset.permission]);
    input.checked?members.add(input.value):members.delete(input.value);
    candidate.source_readers[input.dataset.permission]=[...members].sort(); comparison=undefined;
  }));
  document.querySelectorAll('[data-reviewer]').forEach(input=>input.addEventListener('change',()=>{
    const reviewers=new Set(candidate.approvers); input.checked?reviewers.add(input.dataset.reviewer):reviewers.delete(input.dataset.reviewer);
    candidate.approvers=[...reviewers].sort();comparison=undefined;
  }));
  document.querySelector('#policy-name')?.addEventListener('input',event=>{candidate.name=event.target.value;comparison=undefined;});
  document.querySelector('#policy-lifetime')?.addEventListener('change',event=>{candidate.approval_minutes=Number(event.target.value);comparison=undefined;});
  document.querySelectorAll('[data-input]').forEach(input=>input.addEventListener('change',()=>{
    const inputs=new Set(scenario.inputs);input.checked?inputs.add(input.dataset.input):inputs.delete(input.dataset.input);scenario.inputs=[...inputs].sort();invalidateComparison();
  }));
  document.querySelector('#sim-destination')?.addEventListener('change',event=>{scenario.destination=event.target.value;invalidateComparison();});
  document.querySelector('#sim-approval')?.addEventListener('change',event=>{scenario.approval=event.target.value;invalidateComparison();});
  document.querySelector('#sim-lineage')?.addEventListener('change',event=>{scenario.complete_lineage=event.target.checked;invalidateComparison();});
  document.querySelector('#sim-age')?.addEventListener('input',event=>{const age=approvalAge(event.target.value);event.target.setAttribute('aria-invalid',String(age===null));scenario.approval_age_minutes=age===null?event.target.value:age;invalidateComparison();});
  document.querySelectorAll('[data-preset]').forEach(button=>button.addEventListener('click',()=>{
    selectedPreset=button.dataset.preset;
    scenario={inputs:selectedPreset==='approved'||selectedPreset==='changed'?['approved-planning','directory']:['budget','compensation','directory'],destination:selectedPreset==='hr'?'hr':'engineering',approval:selectedPreset==='changed'?'changed':'valid',approval_age_minutes:0,complete_lineage:true};
    comparison=undefined;render();
  }));
  document.querySelector('#compare')?.addEventListener('click',()=>perform(compare));bindResults();
  document.querySelector('#generate')?.addEventListener('click',()=>perform(async()=>{await command({action:'generate',approved_inputs:false,expected_revision:state.policy_revision});notify('Version 1 recorded. The release gate evaluated its captured inputs.');}));
  document.querySelector('#repair')?.addEventListener('click',()=>perform(async()=>{await command({action:'generate',approved_inputs:true,expected_revision:state.policy_revision});notify('Fresh version created from shared planning data. The restricted draft was excluded.');}));
  document.querySelector('#approve')?.addEventListener('click',()=>perform(async()=>{
    const c=state.current,reviewer=document.querySelector('#reviewer').value;
    await command({action:'approve',reviewer,artifact:c.artifact.id,expected:c.assessment.context_digest});
    notify('Approval recorded for this exact version, destination and policy.');
  }));
  document.querySelector('#release')?.addEventListener('click',()=>perform(async()=>{
    const c=state.current;await command({action:'release',artifact:c.artifact.id,expected:c.assessment.context_digest});
    notify('Release recorded for this exact version and destination.');
  }));
  document.querySelector('#revise')?.addEventListener('click',()=>perform(async()=>{
    await command({action:'revise',artifact:state.current.artifact.id});notify('New artifact recorded. The previous approval no longer matches.');
  }));
}
function render() {
  if (!state) return;
  const id=step(),index=chapters.findIndex(([key])=>key===id);
  document.title=`TrustGraph — ${chapters[index][1]}`;
  document.querySelector('#breadcrumb').textContent=chapters[index][1];
  document.querySelector('#chapters').innerHTML=chapters.map(([key,name],i)=>`<a class="chapter ${key===id?'current':''}" href="#${key}" ${key===id?'aria-current="step"':''}><span class="chapter-number">0${i+1}</span><span>${name}</span><span class="chapter-arrow" aria-hidden="true">↗</span></a>`).join('');
  main.innerHTML=({company:companyPage,policy:policyPage,simulator:simulatorPage,enforce:enforcePage,revision:revisionPage}[id])();bind();
}
document.querySelector('.skip-link').addEventListener('click',event=>{event.preventDefault();main.focus();main.scrollIntoView({block:'start'});});
document.querySelector('#reset').addEventListener('click',()=>perform(async()=>{
  state=await api('/reset',{});candidate=clone(state.policy);comparison=undefined;previousFingerprint=undefined;editingPolicy=false;
  selectedPreset='private';scenario={inputs:['budget','compensation','directory'],destination:'engineering',approval:'missing',approval_age_minutes:0,complete_lineage:true};
  location.hash='company';notify('Story restarted with the initial policy.');
}));
window.addEventListener('hashchange',()=>{render();main.focus({preventScroll:true});window.scrollTo({top:0,behavior:'instant'});});
async function initialize() {
  try {
    const {value, restarted}=await loadInitialState();state=value;candidate=clone(state.policy);render();
    if (restarted) notify('Saved progress was from an older version of this demo, so the story restarted.');
  }
  catch(error) { main.innerHTML=`<div class="error-box"><strong>The workspace could not connect.</strong><p>${escape(error.message)}</p></div>`; }
}
initialize();
