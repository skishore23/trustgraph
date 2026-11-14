let h, host, actor, data, generation = 0;
let chapter = Number(sessionStorage.getItem('trustgraph-demo-chapter') || 1);
if (![1, 2, 3].includes(chapter)) chapter = 1;
let selected = 'release-bundle';
export function configureShowcase(helpers) { h = helpers; }
export function unmountShowcase() { generation++; host = undefined; }
export async function mountShowcase(element, identity) {
  const stamp = ++generation;
  host = element; actor = identity;
  const result = await h.api('/demo');
  if (stamp !== generation) return;
  data = result; render();
}
const e = value => h.escape(value);
const person = id => ({maya: 'Maya', arun: 'Ravi', sam: 'Sam', agent: 'Atlas'}[id] || id);
const statusText = {blocked: 'Blocked', ready: 'Ready to proceed', needs_approval: 'Approval required', needs_approval_renewal: 'Approval needs renewal'};
const tone = status => status === 'blocked' ? 'blocked' : status === 'ready' ? 'ready' : 'waiting';
const chip = (label, style = '') => `<span class="demo-chip ${style}">${e(label)}</span>`;
const button = (action, label, primary = false, disabled = false) => `<button class="button ${primary ? 'primary' : 'secondary'}" data-demo-action="${action}" ${disabled ? 'disabled' : ''}>${e(label)}</button>`;
const identity = (id, label) => `<button class="demo-identity" data-demo-identity="${id}">${h.icon('user', 13)}${e(label || `Preview as ${person(id)}`)}</button>`;
function checks(a) {
  return `<dl class="demo-checks"><div><dt>Information permitted</dt><dd class="${a.information_permitted ? 'ready' : 'blocked'}">${h.icon(a.information_permitted ? 'check' : 'lock', 15)}${a.information_permitted ? 'Yes' : 'No'}</dd></div><div><dt>Approval matches this context</dt><dd class="${a.approval_valid ? 'ready' : 'waiting'}">${h.icon(a.approval_valid ? 'check' : 'clock', 15)}${a.approval_valid ? 'Yes · valid' : a.approval_matches ? 'Matches · expired or invalid' : a.approval_count ? 'No' : 'Required'}</dd></div></dl>`;
}
function evidence(a, title = 'Recorded inputs and exact artifact') {
  return `<details class="demo-evidence"><summary>${e(title)}</summary><p>Fixture generation · version ${a.version}</p><pre>${e(a.content)}</pre><p>Content digest</p><code>${e(a.hash)}</code>${a.transformation ? `<p>Captured inputs</p><ul>${a.transformation.manifest.inputs.map(i => `<li><code>${e(i.artifact)}</code> · ${e(i.role)}</li>`).join('')}</ul><p>Provider: ${e(a.transformation.manifest.provider)}</p>` : ''}</details>`;
}
function shell() {
  const chapters = [['The boundary', 'Can this file be shared?'], ['The approval', 'Which version was approved?'], ['The release', 'What does a change affect?']];
  return `<div class="demo-header"><div><div class="demo-eyebrow">NORTHSTAR BANK / RECONCILIATION MODERNIZATION</div><h1>One agent. Two departments.</h1><p>Ship the payment fix while keeping a confidential investigation inside Risk.</p></div><button class="demo-reset" data-demo-action="reset">${h.icon('refresh', 13)}Reset demo</button></div><div class="demo-people"><span><b>M</b>Maya <small>Engineering lead</small></span><span><b>R</b>Ravi <small>Risk lead</small></span><span><b class="atlas">${h.icon('agent', 13)}</b>Atlas <small>Coding agent</small></span></div><nav class="demo-chapters" aria-label="Demo chapters">${chapters.map(([title, description], i) => `<button data-demo-chapter="${i + 1}" aria-pressed="${chapter === i + 1}"><span class="demo-chapter-number">${i + 1}</span><span><strong>${title}</strong><small>${description}</small></span></button>`).join('')}</nav><div id="demo-scene"></div><footer class="demo-footer"><span class="demo-live-dot"></span>Local demo sandbox · fictional records · Rust decisions · ${data.publisher_calls} publisher calls<details><summary>What this demo runs</summary><p>The same Rust lineage verifier, immutable versions and human approval commands used by the workspace. This walkthrough has a fixed demo policy and separate in-memory history. Reset clears only that history. The policy overlay never changes active access. Generation and reports are fixtures; no model, tests, branch push or PR runs here. Live HydraDB and Composio qualification are pending.</p></details></footer>`;
}
function simple() {
  const s = data.simple;
  let original;
  if (!s.visible) {
    original = `<section class="demo-access"><span class="demo-lock">${h.icon('lock', 24)}</span><h3>Risk-derived work stays with Risk</h3><p>${e(s.reason)}</p>${identity('arun', 'Preview as Ravi to follow the story')}</section>`;
  } else {
    const blocked = s.simulated && !s.assessment.information_permitted;
    original = `<section class="demo-card"><div class="demo-card-top"><h3>Atlas’s proposed test</h3>${chip('Recorded generation')}</div><div class="demo-simple-map"><div class="demo-inputs"><div class="demo-mini-node">${h.icon('files', 16)}<strong>Engineering specification</strong>${chip('Engineering + Risk')}</div><div class="demo-mini-node ${blocked ? 'restricted' : ''}">${h.icon('lock', 16)}<strong>Confidential investigation</strong>${chip('Risk only', 'blocked')}</div></div><div class="demo-flow-down">↓ <span>Used as input · captured task instruction also recorded</span></div><div class="demo-generation">${h.icon('agent', 16)}Atlas generation</div><div class="demo-flow-down">↓ <span>Produces</span></div><div class="demo-output-row"><div class="demo-mini-node ${blocked ? 'restricted' : ''}">${h.icon('files', 17)}<strong>Duplicate-payment test</strong><small>No visible quotation is needed for provenance to matter.</small></div><span class="demo-arrow">→</span><div class="demo-mini-node ${blocked ? 'restricted' : ''}">${h.icon('target', 17)}<strong>Engineering repository</strong><small>Maya + Sam can read this repository</small></div></div></div><div class="demo-actions">${button('simulate_mixed', 'Simulate publishing', true)}${button('fresh_generation', 'Start fresh with Engineering inputs', false, !s.simulated || !!data.fresh)}</div>${s.simulated ? `<div class="demo-result blocked" role="status"><div class="demo-result-title">${h.icon('lock', 18)}<strong>Engineering cannot receive this file</strong></div><p>The file depends on a Risk-only source. The repository includes Engineering readers who cannot read that source. A human approval cannot expand source access.</p><div class="demo-path">${s.path.map(p => `<span>${e(p)}</span>`).join('<b>→</b>')}</div>${checks(s.assessment)}</div>` : '<p class="demo-hint">Atlas may read both sources. Simulate whether Engineering may receive the resulting file.</p>'}${evidence(s.mixed)}</section>`;
  }
  return `<div class="demo-intro"><span>CHAPTER 1 / THE WRONG INFORMATION, THE RIGHT PROJECT</span><h2>Can this test enter Engineering?</h2><p>Engineering needs a regression test for duplicate payments. Atlas used a confidential fraud investigation to help create it. The test does not visibly quote the investigation.</p></div>${original}${data.fresh ? `<section class="demo-card demo-repair"><div class="demo-card-top"><h3>A fresh generation, a different context</h3>${chip('Information permitted', 'ready')}</div><p>Atlas started again with the Engineering specification and captured instruction only. The old test, investigation and conversation are absent from the new manifest.</p><div class="demo-safe-path">Engineering specification → Fresh generation → New test → Engineering</div>${checks(data.fresh.assessment)}<p class="demo-hint">This new fixture may miss useful Risk knowledge. We do not claim it is equivalent. Ravi may need to provide an authorized specification.</p>${evidence(data.fresh.artifact)}<div class="demo-actions"><button class="button primary" data-demo-next="2">Continue to the approval →</button></div></section>` : ''}`;
}
function medium() {
  const m = data.medium, a = m.assessment;
  const approvedVersion = m.approvals.length ? data.medium.approvals.at(-1).context.artifact : undefined;
  return `<div class="demo-intro"><span>CHAPTER 2 / THE APPROVAL IS REAL, THE VERSION CHANGED</span><h2>Does the approval cover this patch?</h2><p>Maya reviews an Engineering-safe change before Atlas opens a draft PR. A small edit creates a new deliverable. Her earlier approval remains in history.</p></div><section class="demo-card"><div class="demo-card-top"><h3>Duplicate-payment handling</h3>${chip(`Patch v${m.artifact.version}`)}</div><div class="demo-review-meta"><span>${h.icon('target', 14)}${m.audience.id === 'engineering-repo' ? 'Engineering repository' : 'Risk review room'}</span><span>${h.icon('user', 14)}Reviewing as ${person(actor)}</span></div><pre class="demo-code">${e(m.artifact.content)}</pre><div class="demo-review-callout">${h.icon('shield', 17)}<p>Approval binds this exact content and destination. Proposed code is shown for review; this walkthrough does not run or attest its tests.</p></div><div class="demo-actions">${button('approve_patch', `Approve patch v${m.artifact.version}`, true, !data.can_review || a?.approval_valid)}${button('change_patch', 'Change patch', false, !m.approvals.length)}${button('simulate_patch', 'Simulate publishing')}${button('change_destination', 'Change destination', false, !m.approvals.length)}</div>${!data.can_review ? `<p class="demo-hint">Agents cannot approve. ${identity('maya', 'Preview as Maya to review')}</p>` : actor !== 'maya' ? `<p class="demo-hint">${person(actor)} is also an authorized demo reviewer. ${identity('maya', 'Use Maya for this chapter')}</p>` : ''}${a ? `<div class="demo-result ${tone(a.status)}" role="status"><div class="demo-result-title">${h.icon(a.status === 'ready' ? 'check' : 'shield', 18)}<strong>${a.status === 'ready' ? 'This exact patch is ready for the gate' : a.approval_matches ? 'Approval matches, but is no longer valid' : a.approval_count ? 'Approval no longer matches' : 'A human approval is still required'}</strong></div>${checks(a)}${a.changed.length ? `<p>Changed since a recorded approval: <strong>${a.changed.map(e).join(', ')}</strong>.</p>` : ''}<p>${a.status === 'ready' ? 'Both checks pass in the sandbox. No branch or PR has been created.' : m.reviewer_authorized && a.approval_count ? 'Maya’s reviewer authority is still valid. Review the current patch and destination to issue a new exact approval.' : 'Review the content above and approve this version.'}</p></div>` : `<p class="demo-hint">${m.approvals.length ? 'The proposed context changed. Simulate publishing to check the saved approval.' : 'Inspect patch v1, then approve it before making an edit.'}</p>`}<details class="demo-evidence"><summary>Approval history · ${m.approvals.length} recorded</summary>${m.approvals.map(ap => `<div class="demo-approval-entry"><strong>${person(ap.signer)} approved ${e(ap.context.artifact)}</strong><p>${e(ap.context.audience.destination)}</p><code>${e(ap.context.digest || ap.context.artifact_hash)}</code></div>`).join('') || '<p>No approval yet.</p>'}<p>Historical approvals are preserved. Current patch: ${e(m.artifact.id)}${approvedVersion ? ` · last approved artifact: ${e(approvedVersion)}` : ''}.</p></details>${evidence(m.artifact)}<div class="demo-actions demo-continue"><button class="button secondary" data-demo-next="3">Continue to release impact →</button></div></section>`;
}
function nodeStatus(n) {
  if (n.needs_recheck) return ['Needs recheck', 'recheck'];
  if (n.kind === 'source') return [n.readers?.includes('maya') ? 'Engineering + Risk' : 'Risk only', n.readers?.includes('maya') ? '' : 'blocked'];
  if (n.publication_candidate) return [statusText[n.assessment.status], tone(n.assessment.status)];
  return n.assessment.information_permitted ? ['Permitted inputs', 'ready'] : ['Blocked for Engineering', 'blocked'];
}
function graphNode(n) {
  const [label, style] = nodeStatus(n);
  return `<button class="demo-graph-node ${style} ${selected === n.id ? 'selected' : ''}" data-demo-node="${n.id}" id="graph-${n.id}" aria-pressed="${selected === n.id}"><span class="demo-node-type">${n.kind === 'source' ? h.icon('files', 13) + 'Source' : h.icon('agent', 13) + 'Generated fixture'}</span><strong>${e(n.title)}</strong>${chip(label, style)}</button>`;
}
function complex() {
  const c = data.complex;
  const byId = Object.fromEntries(c.nodes.map(n => [n.id, n]));
  const release = c.candidates.find(n => n.artifact === 'release-bundle');
  const a = release.assessment;
  const hasApprovals = c.approvals.length > 0;
  const rows = [['case-analysis', 'api-spec'], ['regression-fixtures', 'compatibility-patch'], ['regression-report', 'compatibility-report'], ['release-bundle']];
  const visible = byId[selected] || byId['release-bundle'];
  const selection = c.changed_source || 'case-analysis';
  const rechecking = c.phase === 'marked';
  return `<div class="demo-intro"><span>CHAPTER 3 / ONE ACCESS CHANGE, AN ENTIRE RELEASE</span><h2>What changed? What can still proceed?</h2><p>Two branches feed one release bundle. Ravi now needs to restrict a shared source to Risk because it is part of an active investigation. Nothing has been pushed.</p></div><section class="demo-card demo-impact-controls"><div><h3>Try a hypothetical access change</h3><p>Active source permissions and saved approvals remain untouched.</p></div><div class="demo-policy-controls"><label for="demo-source">Source<select id="demo-source" ${rechecking ? 'disabled' : ''}><option value="case-analysis" ${selection === 'case-analysis' ? 'selected' : ''}>Case analysis</option><option value="api-spec" ${selection === 'api-spec' ? 'selected' : ''}>API specification</option></select></label><div class="demo-readers">Readers<span>Engineering + Risk → <b>Risk only</b></span></div><div class="demo-actions">${c.phase === 'baseline' ? button('approve_baseline', hasApprovals ? 'Refresh baseline approvals' : 'Approve 4 baseline candidates', false, !data.can_review) : button('clear_impact', 'Restore baseline')}${button('mark_impact', 'See impact', c.phase !== 'marked', !hasApprovals || rechecking)}${rechecking ? button('evaluate_impact', 'Reevaluate affected work', true) : ''}</div></div>${!hasApprovals ? `<p class="demo-hint">Review the four publication candidates below, then approve the baseline to compare which approvals survive. ${!data.can_review ? identity('maya', 'Preview as Maya to review') : ''}</p>` : ''}</section>${c.phase !== 'baseline' ? `<div class="demo-impact-banner ${rechecking ? 'recheck' : 'evaluated'}" role="status"><strong>${rechecking ? `${c.affected.length} records need rechecking` : `Access change evaluated across ${c.affected.length} records`}</strong><p>${rechecking ? 'Follow recorded input edges forward. Affected does not yet mean denied. Reevaluate to get the permission and approval results.' : 'Dependent Engineering work is blocked. A Risk-only audience may still be permitted. Work on unrelated branches keeps its own approval.'}</p></div>` : ''}<section class="demo-card demo-map-card"><div class="demo-card-top"><h3>Reconciliation release map</h3>${chip(c.phase === 'baseline' ? 'Baseline' : rechecking ? 'Needs recheck' : 'Hypothetical result', rechecking ? 'recheck' : '')}</div><div class="demo-graph" id="demo-graph"><svg class="demo-connectors" aria-hidden="true"></svg>${rows.map((ids, i) => `<div class="demo-level"><span class="demo-level-label">${i + 1}</span><div class="demo-level-nodes ${ids.length === 1 ? 'single' : ''}">${ids.map(id => graphNode(byId[id])).join('')}</div></div>`).join('')}<div class="demo-level"><span class="demo-level-label">5</span><div class="demo-level-nodes single"><div class="demo-pending-pr ${release.needs_recheck ? 'recheck' : tone(a.status)}" id="graph-pending-pr"><span>${h.icon('shield', 14)}Exact-context approval → intended destination</span><strong>Pending Engineering PR</strong>${chip(release.needs_recheck ? 'Needs recheck' : statusText[a.status], release.needs_recheck ? 'recheck' : tone(a.status))}</div></div></div></div><div class="demo-map-legend"><span><b>→</b> Used as input</span><span><b>┄</b> Approval + intended target</span><span>Reports are fixtures, not test attestations</span></div><div class="demo-side-branches">${['risk-note', 'documentation-fix'].map(id => `<div><p>${e(c.edges.find(edge => edge.to === id)?.from === 'case-analysis' ? 'Case analysis' : 'API specification')} <span>→ used as input</span></p>${graphNode(byId[id])}</div>`).join('')}</div></section><div class="demo-candidate-heading"><h3>Four publication candidates</h3><span>Permission and approval are checked separately</span></div><div class="demo-candidates">${c.candidates.map(candidate => { const n = byId[candidate.artifact], a = candidate.assessment; return `<article class="demo-card demo-candidate ${candidate.needs_recheck ? 'recheck' : tone(a.status)}"><div class="demo-card-top"><h3>${e(n.title)}</h3>${chip(candidate.needs_recheck ? 'Needs recheck' : statusText[a.status], candidate.needs_recheck ? 'recheck' : tone(a.status))}</div><p class="demo-candidate-target">${h.icon('target', 13)}${candidate.audience === 'risk-room' ? 'Risk review room' : 'Engineering repository'}</p>${candidate.needs_recheck ? '<p class="demo-hint">Awaiting reevaluation. No final decision yet.</p>' : checks(a)}${a.changed.length && !candidate.needs_recheck ? `<p class="demo-hint">Changed: ${a.changed.map(e).join(', ')}.</p>` : ''}${evidence(n.artifact, 'Review candidate and captured inputs')}</article>`; }).join('')}</div><section class="demo-card demo-inspector"><div class="demo-card-top"><h3>Inspect: ${e(visible.title)}</h3>${chip(c.phase === 'baseline' ? 'Baseline context' : visible.affected ? 'In changed source’s descendants' : 'Independent of this change')}</div>${evidence(visible.artifact, 'Exact artifact and captured manifest')}<details class="demo-evidence"><summary>Typed relationships and decision evidence</summary><pre>${e(JSON.stringify({input_edges: c.edges.filter(edge => edge.from === visible.id || edge.to === visible.id), approval_edges: c.approval_edges.filter(edge => edge.from === visible.id), target_edges: c.target_edges.filter(edge => edge.from === visible.id), assessment: visible.needs_recheck ? 'Needs recheck; no final decision yet' : visible.assessment}, null, 2))}</pre></details></section>`;
}
function render() {
  if (!host?.isConnected) return;
  host.innerHTML = `<div class="demo-workspace">${shell()}</div>`;
  host.querySelector('#demo-scene').innerHTML = chapter === 1 ? simple() : chapter === 2 ? medium() : complex();
  host.querySelectorAll('[data-demo-chapter],[data-demo-next]').forEach(b => b.onclick = () => {
    chapter = Number(b.dataset.demoChapter || b.dataset.demoNext);
    sessionStorage.setItem('trustgraph-demo-chapter', chapter); render();
    host.querySelector('.demo-chapters').scrollIntoView({block: 'start', behavior: 'smooth'});
  });
  host.querySelectorAll('[data-demo-identity]').forEach(b => b.onclick = () => h.switchActor(b.dataset.demoIdentity));
  host.querySelectorAll('[data-demo-node]').forEach(b => b.onclick = () => { selected = b.dataset.demoNode; render(); host.querySelector(`[data-demo-node="${selected}"]`)?.focus({preventScroll: true}); });
  host.querySelectorAll('[data-demo-action]').forEach(b => b.onclick = async () => {
    const stamp = generation;
    const action = b.dataset.demoAction;
    const body = {action};
    if (action === 'approve_patch') body.expected = data.medium.expected;
    if (action === 'approve_baseline') body.expected = data.complex.expected;
    if (action === 'mark_impact') body.source = host.querySelector('#demo-source').value;
    host.querySelectorAll('[data-demo-action]').forEach(control => control.disabled = true);
    try {
      const result = await h.api(action === 'reset' ? '/demo/reset' : '/demo', action === 'reset' ? {} : body);
      if (stamp !== generation) return;
      data = result;
      if (action === 'reset') { chapter = 1; selected = 'release-bundle'; sessionStorage.setItem('trustgraph-demo-chapter', 1); }
      render();
      const focus = host.querySelector(`[data-demo-action="${action}"]:not(:disabled)`) || host.querySelector('.demo-result') || host.querySelector('.demo-intro h2');
      if (focus && !focus.matches('button')) focus.tabIndex = -1;
      focus?.focus({preventScroll: true});
      if (action === 'fresh_generation') host.querySelector('.demo-repair')?.scrollIntoView({behavior: 'smooth', block: 'start'});
    } catch (error) { if (stamp === generation) { render(); h.toast(error.message, true); } }
  });
  if (chapter === 3) drawEdges();
}
function drawEdges() {
  const graph = host.querySelector('#demo-graph'), svg = graph.querySelector('svg');
  const rect = graph.getBoundingClientRect();
  svg.setAttribute('viewBox', `0 0 ${rect.width} ${rect.height}`);
  const edges = [...data.complex.edges, {from: 'release-bundle', to: 'pending-pr', kind: 'approval_target'}];
  svg.innerHTML = edges.map(edge => {
    const from = graph.querySelector(`#graph-${edge.from}`), to = graph.querySelector(`#graph-${edge.to}`);
    if (!from || !to) return '';
    const f = from.getBoundingClientRect(), t = to.getBoundingClientRect();
    const x1 = f.left + f.width / 2 - rect.left, y1 = f.bottom - rect.top;
    const x2 = t.left + t.width / 2 - rect.left, y2 = t.top - rect.top;
    const affected = data.complex.affected.includes(edge.from) && (edge.kind === 'approval_target' || data.complex.affected.includes(edge.to));
    return `<path d="M ${x1} ${y1} C ${x1} ${(y1+y2)/2}, ${x2} ${(y1+y2)/2}, ${x2} ${y2}" class="${affected ? 'affected' : ''} ${edge.kind === 'approval_target' ? 'approval-edge' : ''}"/>`;
  }).join('');
}
let resizeTimer;
window.addEventListener('resize', () => { clearTimeout(resizeTimer); resizeTimer = setTimeout(() => { if (host?.isConnected && chapter === 3) drawEdges(); }, 80); });
