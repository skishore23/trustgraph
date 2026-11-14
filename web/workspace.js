const app = document.querySelector('#app');
let state, graphState, timer, lastGraphCheck = 0;
let selected = location.hash.split('/')[2];
let goal = 'Fix valid_amount so zero-value payment authorizations are accepted while negative values remain invalid. Add a regression test for zero and preserve the existing tests. Only change src/lib.rs.';
let simulationPolicy, simResult, simDraft, mapOpen = false, inspectingPlan = false;
const names = { queued: 'Queued', running: 'Agent working', awaiting_review: 'Ready for review', approved: 'Approved', published: 'PR opened', blocked: 'Stopped', interrupted: 'Interrupted', outcome_unknown: 'Publication unconfirmed', publication_stopped: 'Publication stopped', gate_passed: 'Gate passed · action simulated' };
const e = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const clone = value => JSON.parse(JSON.stringify(value));
const page = () => location.hash.split('/')[1] || 'policy';
const badge = (label, kind = '') => `<span class="badge ${kind}">${e(label)}</span>`;
const heading = (title, description) => `<div class="heading"><h1>${e(title)}</h1><p>${e(description)}</p></div>`;
async function api(path, body) {
  const response = await fetch(`/api${path}`, { method: body ? 'POST' : 'GET', headers: body ? { 'Content-Type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || `Request failed (${response.status})`);
  return data;
}
function notify(text) {
  const notice = document.querySelector('#notice');
  notice.textContent = text; notice.hidden = false;
  setTimeout(() => notice.hidden = true, 7000);
}
function story() {
  return `<section class="story-strip" aria-label="Walkthrough"><strong>Payment authorization release</strong><p>A zero-value authorization is being rejected. Fix the bug without releasing an unreviewed change.</p><ol>${[['policy', 'Set the boundary'], ['simulator', 'Challenge the policy'], ['agent', 'Run and review Codex']].map(([route, label], i) => `<li><a href="#/${route}" ${page() === route ? 'aria-current="step"' : ''}><span>${i + 1}</span>${label}</a></li>`).join('')}</ol></section>`;
}
function architecture() {
  const count = graphState?.records?.reduce((n, r) => n + Number(r.records || 0), 0) || 0;
  const latest = state.jobs[0];
  return `<details class="system-map" id="system-map" ${mapOpen ? 'open' : ''}><summary>How TrustGraph works ${badge(graphState?.connected ? 'HydraDB connected' : 'HydraDB unavailable', graphState?.connected ? 'good' : 'bad')}</summary><div class="map-top"><a href="#/agent"><strong>Coding agent</strong><small>HTTP intake · Codex</small></a><span>→</span><button class="map-node" id="inspect-plan"><strong>Plan + input manifest</strong><small>${state.sources.length} registered source files</small></button><span>→</span><a href="#/policy"><strong>Rust checks</strong><small>Policy · lineage · approval</small></a></div><div class="map-bottom"><a href="#/simulator"><strong>Simulator</strong><small>Impact + counterexample</small></a><span class="branch">↙ &nbsp; ↘</span><a href="#/agent"><strong>Runtime</strong><small>Coordinator → publication gate → simulated action</small></a></div><div class="hydra-core"><div><strong>HydraDB</strong><span>trustgraph / trustgraph-governance</span></div><p>Tracks which inputs produced a patch and which approval permits its release.</p>${badge(`${count} live records`)}</div><details class="map-plan"><summary>What the HydraDB records mean</summary><p class="muted">Source → exact input bytes. Manifest → inputs supplied to Codex. Artifact → generated patch. Checks → executed test receipt. Approval → exact patch and policy. Outcome → gate decision. Rust evaluates permissions; HydraDB stores and returns the connected evidence.</p></details>${inspectingPlan ? `<details open class="map-plan"><summary>Captured execution plan</summary><pre>${e(JSON.stringify(latest?.plan?.input_manifest ? latest.plan : { sources: state.sources, policy: state.policy.name }, null, 2))}</pre></details>` : ''}</details>`;
}
function sources() {
  return `<details class="sources"><summary>${state.sources.length} captured source files · stored in HydraDB</summary>${state.sources.map(s => `<p>${e(s.path.split('@')[0])}<small>SHA-256 ${e(s.hash.slice(0, 16))}… · Knowledge: ${e(graphState?.knowledge?.find(k => k.id === s.id)?.indexing_status || s.knowledge_status || 'not registered')}</small></p>`).join('')}<p>Knowledge files are optional browsing copies. The generation broker uses exact bytes from the governance graph.</p></details>`;
}
function policyFields(policy, prefix, live = false) {
  return `<label for="${prefix}-name">Policy name</label><input id="${prefix}-name" value="${e(policy.name)}" required maxlength="100"><label for="${prefix}-paths">Allowed files</label><input id="${prefix}-paths" value="${e(policy.allowed_paths.join(', '))}" required aria-describedby="${prefix}-paths-help"><small class="field-help" id="${prefix}-paths-help">${live ? 'Live workbench: src/lib.rs and README.md. Try other relative paths in the Simulator.' : 'Comma-separated relative paths, for example src/lib.rs, README.md or payments/reconcile.rs. Exact paths; no wildcards.'}</small><div class="fields policy-fields"><label for="${prefix}-max">Maximum changed files<input id="${prefix}-max" type="number" min="1" max="10" value="${policy.max_changed_files}" required></label><label for="${prefix}-minutes">Approval lifetime (minutes)<input id="${prefix}-minutes" type="number" min="1" max="1440" value="${policy.approval_minutes}" required></label></div><label class="check"><input id="${prefix}-tests" type="checkbox" ${policy.require_tests ? 'checked' : ''}> Require passing Rust tests</label>`;
}
function readPolicy(prefix) {
  return { name: document.querySelector(`#${prefix}-name`).value.trim(), allowed_paths: document.querySelector(`#${prefix}-paths`).value.split(',').map(p => p.trim()).filter(Boolean), max_changed_files: Number(document.querySelector(`#${prefix}-max`).value), approval_minutes: Number(document.querySelector(`#${prefix}-minutes`).value), require_tests: document.querySelector(`#${prefix}-tests`).checked };
}
function policy() {
  return heading('Policy', 'You own the release boundary. Allow the payment fix, require tests, and require approval of the exact change.') + `<section class="card"><div class="section-heading"><h2>Active release policy</h2>${badge(`Revision ${state.policy_revision}`)}</div><form id="policy" data-revision="${state.policy_revision}">${policyFields(state.policy, 'policy', true)}<div class="actions"><button type="button" class="secondary" id="test-policy">Test in Simulator →</button><button>Activate policy</button></div><p class="muted">Testing leaves this policy unchanged. Activation creates a new authorization context; existing work must be regenerated and reviewed under it.</p></form></section><section class="card"><h2>Always enforced by Rust</h2><ul>${state.invariants.map(i => `<li>${e(i)}</li>`).join('')}</ul><p class="muted">The live example uses your private workbench repository and its sole owner. Department access controls remain a separate fixture scenario.</p></section>`;
}
function simulator() {
  if (!simDraft) simDraft = { paths: ['src/lib.rs'], tests_passed: true, lineage_complete: true, approval: 'valid', approval_age_minutes: 0 };
  if (!simulationPolicy) simulationPolicy = clone(state.policy);
  return heading('Simulator', 'Try a candidate policy against the same proposed change. Compare decisions before changing the live policy.') + `<p class="scope-note">Supports file scope, file count, required tests and approval lifetime. Inputs and approval below are explicit assumptions. No code executes and no policy is activated.</p><div class="scenario-buttons" aria-label="Proposed change presets">${[['files', 'Code + docs'], ['failed', 'Failed tests'], ['stale', 'Patch changed'], ['policy', 'Policy changed'], ['lineage', 'Missing input'], ['expired', 'Expired approval']].map(([id, label]) => `<button type="button" class="secondary" data-scenario="${id}">${label}</button>`).join('')}</div><form id="simulation"><div class="sim-layout"><section class="card"><div class="section-heading"><h2>1. Candidate policy</h2><button type="button" class="text-button" id="use-active">Use active policy</button></div>${policyFields(simulationPolicy, 'candidate')}<details class="evidence"><summary>Import a policy as JSON</summary><label for="candidate-json">Supported policy JSON</label><textarea id="candidate-json" rows="9" spellcheck="false">${e(JSON.stringify(simulationPolicy, null, 2))}</textarea><button type="button" class="secondary" id="import-policy">Use this JSON</button><p class="muted">Unknown fields and arbitrary English rules are rejected. The built-in lineage and approval guarantees remain enforced.</p><p id="json-error" role="alert"></p></details></section><section class="card"><h2>2. Proposed change</h2><label for="sim-paths">Changed files</label><input id="sim-paths" value="${e(simDraft.paths.join(', '))}"><label for="sim-tests">Test result<select id="sim-tests"><option value="true" ${simDraft.tests_passed ? 'selected' : ''}>Assume passed</option><option value="false" ${simDraft.tests_passed ? '' : 'selected'}>Assume failed</option></select></label><label for="sim-approval">Approval<select id="sim-approval">${[['valid', 'Assume exact approval'], ['stale', 'Patch changed after approval'], ['previous_policy', 'Policy changed after approval'], ['expired', 'Approval expired'], ['missing', 'No approval']].map(([value, label]) => `<option value="${value}" ${simDraft.approval === value ? 'selected' : ''}>${label}</option>`).join('')}</select></label><label for="sim-age">Approval age (minutes)<input id="sim-age" type="number" min="0" max="10080" value="${simDraft.approval_age_minutes || 0}"></label><small class="field-help">Both policies evaluate the same hypothetical approval age against their own lifetime.</small><label class="check"><input type="checkbox" id="sim-lineage" ${simDraft.lineage_complete ? 'checked' : ''}> Input manifest is complete</label><button type="button" class="text-button" id="use-latest" ${state.jobs.some(j => j.change) ? '' : 'disabled'}>Use latest agent change</button><div class="actions"><button>Compare policies</button></div><p class="muted">Checks this proposal against both active and candidate policies. Results and policy relationships are recorded in HydraDB.</p></section></div></form><div id="simulation-result" aria-live="polite">${simulationResult()}</div><details class="evidence"><summary>Run this experiment from Codex</summary><pre>cargo run --locked -p trustgraph-cli -- policy-simulate \\\n  --case examples/payment-change.json \\\n  --policy examples/payment-policy-expanded.json \\\n  --expect allowed</pre><p class="muted">The agent token can test candidate policies. It cannot activate them or approve publication.</p></details>`;
}
const verdict = status => status === 'allowed' ? 'Allowed' : status === 'needs_approval' ? 'Needs approval' : 'Blocked';
function decisionCard(result, title) {
  return `<section class="card verdict-card"><div class="section-heading"><h2>${e(title)}</h2>${badge(verdict(result.status), result.status === 'allowed' ? 'good' : 'bad')}</div><dl><div><dt>Information permitted</dt><dd>${result.information_permitted ? 'Yes' : 'No'}</dd></div><div><dt>Business policy</dt><dd>${result.policy_passed ? 'Pass' : 'Fail'}</dd></div><div><dt>Exact approval</dt><dd>${result.approval_matched ? 'Assumed match' : 'Does not match'}</dd></div></dl>${result.counterexample ? `<div class="callout bad"><strong>${e(result.counterexample.rule)}</strong><p>${e(result.counterexample.explanation)}</p><p class="failure-path">${result.counterexample.path.map(e).join(' → ')}</p></div>` : '<p class="muted">All modeled conditions pass. Runtime still checks the actual patch and current evidence.</p>'}</section>`;
}
function simulationResult() {
  const result = simResult;
  if (!result) return '';
  return `<div class="comparison-heading"><h2>Same proposed change. Two policy decisions.</h2>${badge('Hypothetical · zero publication effects')}</div><div class="sim-layout">${decisionCard(result.baseline, `Active: ${result.active_policy.name}`)}${decisionCard(result, `Candidate: ${result.candidate_policy.name}`)}</div><p class="scope-note">${result.policy_changed ? 'Candidate tested only. Your active policy and existing approvals were not changed.' : 'Candidate matches the active policy. Edit its conditions to explore a different decision.'}${result.active_policy_changed_during_check ? ' The live policy changed during this check; compare again before using the result.' : ''}</p>${result.impact?.length ? `<details class="card"><summary>Impact on ${result.impact.length} existing agent changes</summary><p class="muted">Business-policy projection against saved patches. A changed policy requires a fresh run; the runtime independently rechecks permissions and approval.</p>${result.impact.map(item => `<div class="impact-row"><p>${e(item.goal)}</p>${badge(item.violations.length ? 'Outside candidate policy' : 'Within candidate scope', item.violations.length ? 'bad' : 'good')}<p class="muted">${e(item.violations.join('; ') || (item.requires_fresh_run ? 'A fresh generation and review would be required under this candidate.' : 'Policy context unchanged. Current approval is checked separately.'))}</p></div>`).join('')}</details>` : ''}<div class="actions"><a class="button" href="#/agent">Run the real agent →</a></div><details class="evidence"><summary>Rust decisions and HydraDB evidence</summary><p>Candidate policy: ${e(result.candidate_policy_graph_id)}</p><p>Simulation: ${e(result.graph_id)}</p><pre>${e(JSON.stringify(result, null, 2))}</pre></details>`;
}
function readSimulation() {
  simulationPolicy = readPolicy('candidate');
  simDraft = { paths: document.querySelector('#sim-paths').value.split(',').map(p => p.trim()).filter(Boolean), tests_passed: document.querySelector('#sim-tests').value === 'true', approval: document.querySelector('#sim-approval').value, lineage_complete: document.querySelector('#sim-lineage').checked, approval_age_minutes: Number(document.querySelector('#sim-age').value) };
}
function clearSimulation() {
  simResult = undefined;
  const output = document.querySelector('#simulation-result');
  if (output) output.innerHTML = '';
}
function bindSimulator() {
  const form = document.querySelector('#simulation');
  if (!form) return;
  form.addEventListener('input', () => { readSimulation(); clearSimulation(); });
  document.querySelector('#use-active').onclick = () => { readSimulation(); simulationPolicy = clone(state.policy); clearSimulation(); render(); };
  document.querySelector('#import-policy').onclick = () => {
    try {
      const value = JSON.parse(document.querySelector('#candidate-json').value);
      const keys = ['name', 'allowed_paths', 'max_changed_files', 'require_tests', 'approval_minutes'];
      if (!value || typeof value !== 'object' || Object.keys(value).length !== keys.length || keys.some(k => !Object.hasOwn(value, k)) || !Array.isArray(value.allowed_paths) || value.allowed_paths.some(path => typeof path !== 'string') || typeof value.name !== 'string' || typeof value.require_tests !== 'boolean' || !Number.isInteger(value.max_changed_files) || !Number.isInteger(value.approval_minutes)) throw new Error('Use exactly the five supported policy fields shown in the example.');
      readSimulation(); simulationPolicy = value; clearSimulation(); render();
    } catch (error) { document.querySelector('#json-error').textContent = error.message; }
  };
  document.querySelectorAll('[data-scenario]').forEach(button => button.onclick = () => {
    readSimulation();
    const scenario = button.dataset.scenario;
    simDraft = { paths: scenario === 'files' ? ['src/lib.rs', 'README.md'] : ['src/lib.rs'], tests_passed: scenario !== 'failed', lineage_complete: scenario !== 'lineage', approval: scenario === 'stale' ? 'stale' : scenario === 'policy' ? 'previous_policy' : scenario === 'expired' ? 'expired' : 'valid', approval_age_minutes: scenario === 'expired' ? 1441 : 0 };
    clearSimulation(); render();
  });
  document.querySelector('#use-latest').onclick = () => {
    readSimulation(); const latest = state.jobs.find(j => j.change);
    if (latest) { simDraft.paths = latest.change.files.map(f => f.path); simDraft.tests_passed = latest.change.checks.passed; clearSimulation(); render(); }
  };
  form.onsubmit = async event => {
    event.preventDefault(); readSimulation(); clearSimulation();
    const controls = [...form.elements, ...document.querySelectorAll('[data-scenario]')];
    controls.forEach(control => control.disabled = true);
    const output = document.querySelector('#simulation-result');
    output.innerHTML = '<p role="status">Comparing policies in Rust and recording evidence in HydraDB…</p>';
    output.setAttribute('aria-busy', 'true');
    try { simResult = await api('/simulate', { ...simDraft, policy: simulationPolicy }); output.innerHTML = simulationResult(); output.tabIndex = -1; output.focus({ preventScroll: true }); output.scrollIntoView({ behavior: 'smooth', block: 'start' }); }
    catch (error) { output.innerHTML = `<p class="callout bad" role="alert">${e(error.message)}</p>`; }
    finally { controls.forEach(control => control.disabled = false); output.removeAttribute('aria-busy'); }
  };
}
function work() {
  const job = state.jobs.find(j => j.id === selected);
  if (job) return detail(job);
  return heading('Agent', 'Run a real coding agent, inspect its actual patch and tests, then decide whether to release it.') + `<section class="card"><h2>Fix the zero-value authorization bug</h2><p class="muted">The private workbench rejects <code>valid_amount(0)</code>. Codex should accept zero, reject negatives, preserve positive values and add a regression test.</p><form id="new-work"><label for="goal">Work order</label><textarea id="goal" required maxlength="4000" rows="4">${e(goal)}</textarea>${sources()}<button ${state.jobs.some(j => ['running', 'queued'].includes(j.status)) ? 'disabled' : ''}>Run Codex</button></form><details class="evidence"><summary>Run from Codex or your terminal</summary><pre>cargo run --locked -p trustgraph-cli -- run \\\n  --goal "Fix valid_amount to accept zero, reject negatives and add a zero regression test. Only change src/lib.rs."</pre><p class="muted">Run from TrustGraph. The CLI reads the restricted agent token locally and streams actual run stages. Approval stays in this app.</p></details></section><section class="work-list"><h2>Recent work</h2>${state.jobs.map(j => `<button class="work-row" data-job="${j.id}"><span><strong>${e(j.goal)}</strong><small>${new Date(j.created_at * 1000).toLocaleString()}</small></span>${badge(names[j.status] || j.status, j.status === 'gate_passed' || j.status === 'published' ? 'good' : j.status === 'blocked' ? 'bad' : '')}</button>`).join('') || '<p class="muted">Your agent’s work will appear here.</p>'}</section>`;
}
function detail(job) {
  const change = job.change, ready = job.decision?.status === 'READY', pending = job.decision?.status === 'PENDING_APPROVAL';
  const passed = change?.checks.passed, testCount = change?.checks.output.match(/test result: ok\. (\d+) passed/)?.[1];
  const canReview = (pending || (!ready && job.information_permitted) || (ready && !job.graph_approval_recorded)) && !job.violations.length && job.status !== 'blocked' && !job.operation;
  const canGate = ready && job.graph_approval_recorded && !job.violations.length && job.status !== 'blocked';
  return `<button class="back" id="back">← All work</button>` + heading('Agent run', job.goal) + `<div class="status-row">${badge(names[job.status] || job.status, job.status === 'gate_passed' ? 'good' : '')}<span>${e(state.repository)}</span></div><section class="progress" aria-label="Work progress"><span>HydraDB inputs</span><b>→</b><span>Codex</span><b>→</b><span>Rust tests</span><b>→</b><span>Your review</span><b>→</b><span>Publication gate</span></section>${job.error ? `<div class="callout bad">${e(job.error)}</div>` : ''}${['running', 'queued'].includes(job.status) ? '<section class="card"><h2>Codex is working</h2><p class="muted">The broker loaded exact source bytes from HydraDB into a fresh turn. This view updates with actual activity.</p></section>' : ''}${change ? `<section class="card"><div class="section-heading"><h2>Proposed changes</h2>${badge(`${change.files.length} file${change.files.length === 1 ? '' : 's'}`)}</div>${change.files.map(file => `<details class="file" open><summary>${e(file.path)}</summary><div class="diff"><div><span>Before</span><pre>${e(job.base_files.find(base => base.path === file.path)?.content || 'New file')}</pre></div><div><span>Proposed</span><pre>${e(file.content)}</pre></div></div></details>`).join('')}</section><section class="card"><h2>Release checks</h2><dl><div><dt>Source permission and complete lineage</dt><dd>${job.information_permitted ? 'Passed' : 'Blocked'}</dd></div><div><dt>Business policy</dt><dd>${job.violations.length ? e(job.violations.join('; ')) : 'Passed'}</dd></div><div><dt>Tests for this exact patch</dt><dd class="${passed ? 'good' : 'bad'}">${passed ? `Passed${testCount ? ` · ${testCount} tests` : ''}` : 'Failed'}</dd></div><div><dt>Exact operator approval</dt><dd>${ready ? 'Approved' : job.status === 'published' ? 'Recorded' : job.decision?.violations?.some(v => v.code === 'APPROVAL_EXPIRED') ? 'Expired · review again' : 'Required'}</dd></div></dl><details><summary>View actual test output</summary><p class="muted">${e(change.checks.command)}</p><pre>${e(change.checks.output)}</pre></details><div class="actions">${job.status === 'published' ? `<a class="button" href="${e(job.operation?.receipt?.provider_id)}" target="_blank" rel="noopener">Open draft PR ↗</a>` : `<button data-action="approve" ${canReview ? '' : 'disabled'}>Approve exact change</button><button class="secondary" data-action="gate" ${canGate ? '' : 'disabled'}>Run publication gate</button>`}</div><p class="muted" id="action-status" role="status">${canGate ? 'Approval is recorded. The gate will fetch and validate HydraDB evidence again.' : 'Review the actual patch and test receipt before approving. Approval enables the gate.'}</p><p class="muted">The gate’s default action is simulated. No branch or PR is created.</p><details><summary>Optional: publish a real draft PR</summary><p class="muted">Separate action targeting only the private workbench repository. This is outside the default walkthrough.</p><button class="secondary" data-action="publish" ${canGate ? '' : 'disabled'}>Open real draft PR</button></details></section>` : ''}${sources()}<section class="card"><h2>Live activity</h2><ol class="activity">${(job.events || []).map(event => `<li><time>${new Date(event.at * 1000).toLocaleTimeString()}</time><span>${e(event.message)}</span></li>`).join('')}</ol></section><details class="evidence"><summary>Plan, manifest and HydraDB evidence</summary><pre>${e(JSON.stringify(job.plan, null, 2))}</pre><p>Generation: ${e(job.provider?.provider || 'Pending')}</p><p>Capture: ${e(job.provider?.capture || 'Pending')}</p><p>Patch graph record: ${e(job.graph_id || 'Pending')}</p><pre>${e(JSON.stringify(job.decision, null, 2))}</pre></details>`;
}
async function render() {
  document.querySelectorAll('nav a').forEach(link => { const active = link.hash === `#/${page()}`; link.classList.toggle('active', active); if (active) link.setAttribute('aria-current', 'page'); else link.removeAttribute('aria-current'); });
  document.title = `TrustGraph · ${page() === 'policy' ? 'Policy' : page() === 'simulator' ? 'Simulator' : 'Agent'}`;
  app.innerHTML = story() + architecture() + `<section id="panel">${page() === 'policy' ? policy() : page() === 'simulator' ? simulator() : work()}</section>`;
  document.querySelector('#system-map').addEventListener('toggle', event => mapOpen = event.target.open);
  document.querySelector('#inspect-plan').onclick = () => { inspectingPlan = !inspectingPlan; render(); };
  bindSimulator();
  document.querySelectorAll('[data-job]').forEach(button => button.onclick = () => { location.hash = `/agent/${button.dataset.job}`; });
  document.querySelector('#back')?.addEventListener('click', () => { selected = undefined; location.hash = '/agent'; render(); });
  document.querySelector('#goal')?.addEventListener('input', event => goal = event.target.value);
  document.querySelector('#new-work')?.addEventListener('submit', async event => {
    event.preventDefault(); const button = event.target.querySelector('button'); button.disabled = true; button.textContent = 'Submitting to TrustGraph…';
    try { const job = await api('/work', { goal }); location.hash = `/agent/${job.id}`; }
    catch (error) { notify(error.message); button.disabled = false; button.textContent = 'Run Codex'; }
  });
  document.querySelectorAll('[data-action]').forEach(button => button.onclick = async () => {
    document.querySelectorAll('[data-action]').forEach(control => control.disabled = true);
    document.querySelector('#action-status').textContent = button.dataset.action === 'approve' ? 'Recording exact approval and reading back its connected evidence from HydraDB…' : 'Rechecking current policy, exact approval and HydraDB evidence…';
    try { const job = state.jobs.find(j => j.id === selected); await api(`/work/${job.id}/${button.dataset.action}`, { context_digest: job.context_digest }); await load(); }
    catch (error) { notify(error.message); await load(); }
  });
  document.querySelector('#test-policy')?.addEventListener('click', () => { simulationPolicy = readPolicy('policy'); clearSimulation(); location.hash = '/simulator'; });
  document.querySelector('#policy')?.addEventListener('submit', async event => {
    event.preventDefault(); const button = event.target.querySelector('button:last-of-type'); button.disabled = true;
    try { await api('/policy', { policy: readPolicy('policy'), activate: true, expected_revision: Number(event.target.dataset.revision) }); notify('Policy activated. Existing work requires a fresh run under this context.'); await load(); }
    catch (error) { notify(error.message); button.disabled = false; }
  });
}
async function load() {
  clearTimeout(timer);
  try {
    state = await api('/work');
    if (!graphState || Date.now() - lastGraphCheck > 8000) {
      try { graphState = await api('/graph'); lastGraphCheck = Date.now(); } catch { graphState = { connected: false }; }
    }
    await render();
  } catch (error) { app.innerHTML = heading('Workspace unavailable', error.message === 'workspace authentication required' ? 'Operator sign-in required. Open operator.html in your local state folder, or start TrustGraph to create it.' : error.message); }
  if (state?.jobs.some(j => ['queued', 'running'].includes(j.status)) && page() === 'agent') timer = setTimeout(load, 2500);
}
window.addEventListener('hashchange', () => { selected = location.hash.split('/')[2]; load(); });
async function initialize() {
  if (page() === 'connect') {
    const token = location.hash.split('/')[2]; history.replaceState({}, '', '/#/policy'); selected = undefined;
    try { const response = await fetch('/api/session', { method: 'POST', headers: { Authorization: `Bearer ${token}` } }); if (!response.ok) throw new Error('Operator sign-in failed'); }
    catch (error) { app.textContent = error.message; return; }
  }
  await load();
}
initialize();
