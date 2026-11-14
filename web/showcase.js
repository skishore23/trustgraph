let h, host, actor, data, generation = 0;
let chapter = Number(sessionStorage.getItem('trustgraph-demo-chapter') || 1);
if (![1, 2, 3].includes(chapter)) chapter = 1;
let selected;

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
const statusText = {blocked: 'Blocked', ready: 'Ready', needs_approval: 'Needs approval', needs_approval_renewal: 'Renew approval'};
const tone = status => status === 'blocked' ? 'blocked' : status === 'ready' ? 'ready' : 'waiting';
const chip = (label, style = '') => `<span class="demo-chip ${style}">${e(label)}</span>`;
const button = (action, label, primary = true, disabled = false) => `<button class="button ${primary ? 'primary' : 'secondary'}" data-demo-action="${action}" ${disabled ? 'disabled' : ''}>${e(label)}</button>`;
const next = (number, label) => `<button class="button primary" data-demo-next="${number}">${e(label)} ${h.icon('arrow', 14)}</button>`;
const identity = (id, label) => `<button class="button secondary" data-demo-identity="${id}">${h.icon('user', 14)}${e(label || `Preview as ${person(id)}`)}</button>`;
const version = id => String(id).match(/v(\d+)$/)?.[1] || 'previous';

function checks(a) {
  return `<dl class="demo-checks"><div><dt>Information permitted</dt><dd class="${a.information_permitted ? 'ready' : 'blocked'}">${h.icon(a.information_permitted ? 'check' : 'lock', 15)}${a.information_permitted ? 'Yes' : 'No'}</dd></div><div><dt>Approval matches</dt><dd class="${a.approval_valid ? 'ready' : 'waiting'}">${h.icon(a.approval_valid ? 'check' : 'clock', 15)}${a.approval_valid ? 'Yes' : a.approval_matches ? 'Expired or invalid' : a.approval_count ? 'No' : 'Required'}</dd></div></dl>`;
}
function artifactEvidence(a) {
  return `<div class="demo-artifact-evidence"><h4>Version ${a.version}</h4><pre>${e(a.content)}</pre>${a.transformation ? `<h4>Recorded inputs</h4><ul>${a.transformation.manifest.inputs.map(i => `<li>${e(i.artifact)} <span>${e(i.role)}</span></li>`).join('')}</ul>` : ''}<details><summary>Content fingerprint</summary><code>${e(a.hash)}</code></details></div>`;
}
function disclosure(title, body) {
  return `<details class="demo-details"><summary>${e(title)}</summary><div>${body}</div></details>`;
}
function intro(title, description) {
  return `<div class="demo-intro"><h2>${e(title)}</h2><p>${e(description)}</p></div>`;
}
function shell() {
  const chapters = ['Information', 'Approval', 'Impact'];
  return `<div class="demo-header"><div><span class="demo-eyebrow">NORTHSTAR BANK</span><h1>Payment reconciliation</h1><p>One coding agent working across Engineering and Risk.</p></div><button class="demo-reset" data-demo-action="reset">${h.icon('refresh', 14)}Reset</button></div>
    <nav class="demo-chapters" aria-label="Demo chapters">${chapters.map((title, i) => `<button data-demo-chapter="${i + 1}" aria-pressed="${chapter === i + 1}"><span>${i + 1}</span>${title}</button>`).join('')}</nav>
    <div id="demo-scene"></div>
    <footer class="demo-footer"><span>Local demo · Synthetic generation · No publishing</span><details><summary>About this demo</summary><p>Decisions come from the Rust verifier. This sandbox has a fixed demo policy and separate history from Reviews and Policy. Reset clears only sandbox history. Reports are fixtures, not executed tests. No model, branch push or PR runs here. Live HydraDB and Composio integration are pending.</p></details></footer>`;
}

function simple() {
  const s = data.simple;
  const heading = intro('Can Engineering receive this test?', 'Atlas used an Engineering specification and a confidential Risk investigation to create a duplicate-payment test.');
  if (!s.visible) return `${heading}<section class="demo-access">${h.icon('lock', 25)}<h3>This work is restricted to Risk</h3><p>Maya cannot inspect a test derived from the investigation.</p>${identity('arun', 'Preview as Ravi, Risk lead')}</section>`;
  const fresh = data.fresh;
  const blocked = s.simulated && !s.assessment.information_permitted;
  const graphTone = fresh ? 'ready' : blocked ? 'blocked' : '';
  const artifact = fresh ? fresh.artifact : s.mixed;
  return `${heading}<section class="demo-card"><div class="demo-card-heading"><h3>${fresh ? 'A fresh test, using safe inputs' : 'Proposed regression test'}</h3>${chip(fresh ? 'Fresh context' : 'Atlas · coding agent')}</div>
    <div class="demo-simple-map"><div class="demo-inputs ${fresh ? 'single' : ''}"><div class="demo-source">${h.icon('files', 17)}<div><strong>Engineering specification</strong><small>Engineering + Risk</small></div></div>${fresh ? '' : `<div class="demo-source ${blocked ? 'blocked' : ''}">${h.icon('lock', 17)}<div><strong>Risk investigation</strong><small>Risk only</small></div></div>`}</div>
    <div class="demo-flow-label">↓ <span>Atlas generates from these inputs</span></div>
    <div class="demo-output-row"><div class="demo-output ${graphTone}">${h.icon('files', 18)}<strong>${fresh ? 'New regression test' : 'Regression test'}</strong></div><span class="demo-arrow">→</span><div class="demo-output ${graphTone}">${h.icon('user', 18)}<strong>Engineering</strong><small>Maya and her team</small></div></div></div>
    ${fresh ? `<div class="demo-result ready" role="status"><strong>${h.icon('check', 17)}Safe inputs. Ready for human review.</strong><p>The new context excludes the investigation and the old test. It may miss useful Risk knowledge; the two outputs are not claimed to be equivalent.</p></div>` : blocked ? `<div class="demo-result blocked" role="status"><strong>${h.icon('lock', 17)}Blocked: this test depends on Risk-only information.</strong><p>Engineering cannot read one of its sources. Approval cannot override that boundary.</p></div>` : '<p class="demo-hint">Atlas can read both documents. That does not mean it can share everything it creates.</p>'}
    <div class="demo-actions">${fresh ? next(2, 'Continue to approval') : blocked ? button('fresh_generation', 'Generate with safe inputs') : button('simulate_mixed', 'Check permissions')}</div>
    ${disclosure('View evidence', artifactEvidence(artifact) + (fresh ? disclosure('Original blocked test', artifactEvidence(s.mixed)) : ''))}</section>`;
}

function medium() {
  const m = data.medium, a = m.assessment;
  const approved = a?.approval_valid;
  const needsCheck = m.approvals.length > 0 && !m.simulated;
  let actions;
  if (!data.can_review && !needsCheck && !approved) actions = identity('maya', 'Preview as Maya to approve');
  else if (needsCheck) actions = button('simulate_patch', 'Check approval');
  else if (approved) actions = button('change_patch', 'Change patch', m.artifact.version === 1) + (m.artifact.version > 1 ? next(3, 'Continue to impact') : '');
  else actions = button('approve_patch', `Approve version ${m.artifact.version}`);
  const history = `<h4>Approval history</h4>${m.approvals.length ? `<ul>${m.approvals.map(ap => `<li>${e(person(ap.signer))} approved version ${e(version(ap.context.artifact))} <span>${ap.context.audience.id === 'engineering-repo' ? 'Engineering' : 'Risk'}</span></li>`).join('')}</ul>` : '<p>No approval recorded.</p>'}`;
  return `${intro('Does the approval cover this version?', 'Maya approves a safe patch. Atlas makes a small edit. The original approval stays in history.')}
    <section class="demo-card"><div class="demo-card-heading"><h3>Duplicate-payment handling</h3>${chip(`Version ${m.artifact.version}`)}</div><p class="demo-meta">${h.icon('user', 14)}For ${m.audience.id === 'engineering-repo' ? 'Engineering' : 'Risk'} · reviewing as ${e(person(actor))}</p>
    <pre class="demo-code">${e(m.artifact.content)}</pre>
    ${a ? `<div class="demo-result ${tone(a.status)}" role="status"><strong>${h.icon(approved ? 'check' : 'shield', 17)}${approved ? 'This exact version is approved.' : a.approval_matches ? 'The approval has expired or is invalid.' : a.approval_count ? 'The patch changed. Approval no longer matches.' : 'Human approval is required.'}</strong>${checks(a)}${!approved && a.approval_count ? '<p>The reviewer’s authority has not changed. The edited patch needs a new approval.</p>' : ''}</div>` : `<p class="demo-hint">${needsCheck ? 'Version changed. Check whether the saved approval still applies.' : 'Review this exact patch, then approve it for Engineering.'}</p>`}
    <div class="demo-actions">${actions}</div>${actor !== 'maya' && data.can_review ? `<p class="demo-hint">${identity('maya', 'Use Maya for this chapter')}</p>` : ''}
    ${disclosure('View evidence and approval history', history + artifactEvidence(m.artifact))}</section>`;
}

function nodeStatus(n) {
  if (n.needs_recheck) return ['Needs recheck', 'recheck'];
  if (n.kind === 'source') return [n.readers?.includes('maya') ? 'Shared' : 'Risk only', n.readers?.includes('maya') ? '' : 'blocked'];
  if (n.publication_candidate) return [statusText[n.assessment.status], tone(n.assessment.status)];
  return n.assessment.information_permitted ? ['Permitted', 'ready'] : ['Blocked', 'blocked'];
}
function graphNode(n) {
  const [label, style] = nodeStatus(n);
  return `<button class="demo-graph-node ${style} ${selected === n.id ? 'selected' : ''}" data-demo-node="${n.id}" id="graph-${n.id}" aria-pressed="${selected === n.id}"><strong>${e(n.title)}</strong>${chip(label, style)}</button>`;
}
function complex() {
  const c = data.complex;
  const byId = Object.fromEntries(c.nodes.map(n => [n.id, n]));
  const release = c.candidates.find(n => n.artifact === 'release-bundle');
  const hasApprovals = c.approvals.length > 0;
  const marked = c.phase === 'marked';
  const evaluated = c.phase === 'evaluated';
  const rows = [['case-analysis', 'api-spec'], ['regression-fixtures', 'compatibility-patch'], ['regression-report', 'compatibility-report'], ['release-bundle']];
  let action, explanation;
  if (!hasApprovals) {
    action = data.can_review ? button('approve_baseline', 'Approve baseline') : identity('maya', 'Preview as Maya to approve');
    explanation = 'Approve four work items first, so you can see which approvals survive the change.';
  } else if (c.phase === 'baseline') {
    action = button('mark_impact', 'Restrict case analysis to Risk');
    explanation = 'Try an access change. The active policy will stay unchanged.';
  } else if (marked) {
    action = button('evaluate_impact', 'Recheck affected work');
    explanation = `${c.affected.length} records need rechecking. Affected does not automatically mean blocked.`;
  } else {
    action = button('clear_impact', 'Restore baseline', false);
    explanation = 'The same access change leads to different outcomes for different work.';
  }
  const baselineReview = disclosure('Review the four work items', c.candidates.map(candidate => `<section><h4>${e(byId[candidate.artifact].title)} · ${candidate.audience === 'risk-room' ? 'Risk' : 'Engineering'}</h4>${artifactEvidence(byId[candidate.artifact].artifact)}</section>`).join(''));
  const inspector = selected && byId[selected];
  return `${intro('What does one access change affect?', 'A shared case analysis feeds a release several steps later. Other work uses an independent API specification.')}
    <section class="demo-control ${evaluated ? 'compact' : ''}"><div><strong>${evaluated ? 'Impact evaluated' : marked ? 'Needs recheck' : 'Case analysis: shared → Risk only'}</strong><p>${explanation}</p></div><div class="demo-actions">${action}</div></section>
    ${!hasApprovals ? baselineReview : ''}
    <section class="demo-card demo-map-card"><div class="demo-card-heading"><h3>Release dependencies</h3>${chip(evaluated ? 'Hypothetical result' : marked ? 'Needs recheck' : 'Baseline', marked ? 'recheck' : '')}</div>
    <div class="demo-graph" id="demo-graph"><svg class="demo-connectors" aria-hidden="true"></svg>${rows.map((ids, i) => `<div class="demo-level"><span class="demo-level-label">${i + 1}</span><div class="demo-level-nodes ${ids.length === 1 ? 'single' : ''}">${ids.map(id => graphNode(byId[id])).join('')}</div></div>`).join('')}<div class="demo-level"><span class="demo-level-label">5</span><div class="demo-level-nodes single"><div class="demo-pending-pr ${release.needs_recheck ? 'recheck' : tone(release.assessment.status)}" id="graph-pending-pr"><strong>Engineering PR</strong>${chip(release.needs_recheck ? 'Needs recheck' : statusText[release.assessment.status], release.needs_recheck ? 'recheck' : tone(release.assessment.status))}</div></div></div></div>
    <div class="demo-side-branches">${['risk-note', 'documentation-fix'].map(id => `<div><span>${id === 'risk-note' ? 'From case analysis → Risk' : 'From API specification → Engineering'}</span>${graphNode(byId[id])}</div>`).join('')}</div>
    <p class="demo-map-caption">Lines follow recorded inputs. The last step requires exact approval. Reports are fixtures.</p>
    ${inspector ? `<div class="demo-inspector"><div class="demo-card-heading"><h3>${e(inspector.title)}</h3><button class="demo-close" data-demo-close aria-label="Close selected item">×</button></div>${inspector.needs_recheck ? '<p>Needs recheck. No final decision yet.</p>' : inspector.assessment ? checks(inspector.assessment) : `<p>Readers: ${inspector.readers.map(person).map(e).join(', ')}</p>`}${disclosure('View recorded evidence', artifactEvidence(inspector.artifact))}</div>` : ''}</section>
    ${evaluated ? `<section class="demo-outcomes" aria-label="Publication outcomes">${c.candidates.map(candidate => {const a=candidate.assessment;return `<div><span><strong>${e(byId[candidate.artifact].title)}</strong><small>${a.information_permitted ? (a.approval_valid ? 'Permission and approval valid' : 'Information permitted · approval needs renewal') : 'Risk-only input cannot reach Engineering'}</small></span>${chip(statusText[a.status],tone(a.status))}</div>`;}).join('')}</section>` : ''}
    ${hasApprovals ? disclosure('View decision evidence', c.candidates.map(candidate => `<section><h4>${e(byId[candidate.artifact].title)}</h4>${candidate.needs_recheck ? '<p>Needs recheck.</p>' : checks(candidate.assessment)}${artifactEvidence(byId[candidate.artifact].artifact)}</section>`).join('')) : ''}`;
}

function render() {
  if (!host?.isConnected) return;
  host.innerHTML = `<div class="demo-workspace">${shell()}</div>`;
  host.querySelector('#demo-scene').innerHTML = chapter === 1 ? simple() : chapter === 2 ? medium() : complex();
  host.querySelectorAll('[data-demo-chapter],[data-demo-next]').forEach(b => b.onclick = () => {
    chapter = Number(b.dataset.demoChapter || b.dataset.demoNext); selected = undefined;
    sessionStorage.setItem('trustgraph-demo-chapter', chapter); render();
    host.querySelector('.demo-chapters').scrollIntoView({block:'nearest',behavior:'smooth'});
  });
  host.querySelectorAll('[data-demo-identity]').forEach(b => b.onclick = () => h.switchActor(b.dataset.demoIdentity));
  host.querySelectorAll('[data-demo-node]').forEach(b => b.onclick = () => {
    selected = selected === b.dataset.demoNode ? undefined : b.dataset.demoNode;
    const id = b.dataset.demoNode;
    render(); host.querySelector(`[data-demo-node="${id}"]`)?.focus({preventScroll:true});
  });
  host.querySelector('[data-demo-close]')?.addEventListener('click', () => {
    const id = selected; selected = undefined; render(); host.querySelector(`[data-demo-node="${id}"]`)?.focus({preventScroll:true});
  });
  host.querySelectorAll('[data-demo-action]').forEach(b => b.onclick = async () => {
    const stamp = generation;
    const action = b.dataset.demoAction;
    const body = {action};
    if (action === 'approve_patch') body.expected = data.medium.expected;
    if (action === 'approve_baseline') body.expected = data.complex.expected;
    if (action === 'mark_impact') body.source = 'case-analysis';
    host.querySelectorAll('[data-demo-action],[data-demo-chapter],[data-demo-next],[data-demo-identity],[data-demo-node]').forEach(control => control.disabled = true);
    try {
      const result = await h.api(action === 'reset' ? '/demo/reset' : '/demo', action === 'reset' ? {} : body);
      if (stamp !== generation) return;
      data = result;
      if (action === 'reset') {chapter = 1; selected = undefined; sessionStorage.setItem('trustgraph-demo-chapter',1);}
      render();
      const focus = host.querySelector('.demo-result') || host.querySelector('.demo-control strong') || host.querySelector('.demo-intro h2');
      if (focus) focus.tabIndex = -1;
      focus?.focus({preventScroll:true});
    } catch (error) {if (stamp === generation) {render(); h.toast(error.message,true);}}
  });
  if (chapter === 3) drawEdges();
}
function drawEdges() {
  const graph = host.querySelector('#demo-graph'), svg = graph.querySelector('svg');
  const rect = graph.getBoundingClientRect();
  svg.setAttribute('viewBox',`0 0 ${rect.width} ${rect.height}`);
  const edges = [...data.complex.edges,{from:'release-bundle',to:'pending-pr',kind:'approval_target'}];
  svg.innerHTML = edges.map(edge => {
    const from = graph.querySelector(`#graph-${edge.from}`), to = graph.querySelector(`#graph-${edge.to}`);
    if (!from || !to) return '';
    const f=from.getBoundingClientRect(), t=to.getBoundingClientRect();
    const x1=f.left+f.width/2-rect.left, y1=f.bottom-rect.top;
    const x2=t.left+t.width/2-rect.left, y2=t.top-rect.top;
    const affected=data.complex.affected.includes(edge.from)&&(edge.kind==='approval_target'||data.complex.affected.includes(edge.to));
    return `<path d="M ${x1} ${y1} C ${x1} ${(y1+y2)/2}, ${x2} ${(y1+y2)/2}, ${x2} ${y2}" class="${affected?'affected':''} ${edge.kind==='approval_target'?'approval-edge':''}"/>`;
  }).join('');
}
let resizeTimer;
window.addEventListener('resize',() => {
  clearTimeout(resizeTimer);
  resizeTimer=setTimeout(() => {if (host?.isConnected&&chapter===3) drawEdges();},80);
});
