let h, generation=0, saved;
export function configureSimulator(helpers){h=helpers;}
export function unmountSimulator(){generation++;}

export async function mountSimulator(host,identity){
  const version=++generation;
  const {api,escape:e,icon}=h;
  host.innerHTML='<div class="loading">Loading the acquisition review…</div>';
  const scenario=await api('/scenario');
  if(version!==generation)return;
  const sources=scenario.sources, people=scenario.people, bank=scenario.kind==='bank';
  const title=id=>sources.find(s=>s.id===id)?.title||(id==='task-prompt'?'Recorded instructions':id);
  let selection=saved?.identity===identity?structuredClone(saved.selection):structuredClone(scenario.defaults);
  // An identity or source-access change cannot retain hidden input choices.
  selection.sources=selection.sources.filter(id=>sources.some(s=>s.id===id));
  let result, sequence=0, preparing=false;
  host.innerHTML=`<div class="decision-app"><div class="sim-eyebrow"><span class="case-dot"></span>${e(scenario.client||'ATLAS CAPITAL')} <span>/</span> ${e(scenario.engagement||'HELIOS ACQUISITION')}<span class="fiction-label">Fictional case</span></div>
    <div class="sim-heading"><div><h1>${bank?'Reconciliation change review':'Helios acquisition review'}</h1><p>${bank?e(scenario.description):'Atlas needs a recommendation before committing <strong>$240m</strong>.<br>Test which evidence can reach the people making the decision.'}</p></div><button class="sim-reset" id="sim-reset">${icon('refresh',13)}Reset case</button></div>
    <div class="decision-context"><div><span>THE DECISION</span><strong>${bank?'May this patch enter Engineering?':'Commit, renegotiate or pause?'}</strong></div><div><span>THE EXPOSURE</span><strong>${bank?'Confidential cross-department context':'32% revenue from one customer'}</strong></div><div><span>${bank?'WORKING SCOPES':'ACCOUNTABLE PARTNER'}</span><strong>${bank?'Engineering + Risk':'Maya · Aster Advisory'}</strong></div></div>
    <div class="sim-workflow" aria-label="Artifact workflow"><span class="current"><b>1</b>Test permissions</span><i>→</i><span><b>2</b>Prepare ${bank?'patch':'memo'}</span><i>→</i><span><b>3</b>${bank?'Human':'Partner'} review</span><i>→</i><span><b>4</b>Share</span></div>
    <div id="sim-outcome" aria-live="polite" aria-atomic="true"></div>
    <div class="sim-inputs"><fieldset class="sim-panel"><legend><span class="sim-step">01</span>Which evidence should the agent use?</legend><p class="sim-panel-help">${bank?'Select the context for a fresh patch.':'Select documents for a fresh recommendation.'}</p><div class="sim-documents">${sources.map(s=>`<label class="sim-document"><input type="checkbox" name="evidence" value="${e(s.id)}" ${selection.sources.includes(s.id)?'checked':''}><span class="sim-doc-icon">${icon('files',19)}</span><span class="sim-doc-copy"><strong>${e(s.title)}</strong><small>${e(s.owner)}</small><span class="sim-classification">${e(s.classification)}</span></span><a href="#/sources/${encodeURIComponent(s.id)}" class="sim-source-link" aria-label="Read ${e(s.title)}" title="Read source">↗</a></label>`).join('')}</div><p class="sim-instruction">${icon('lock',12)}Recorded instructions are always included.</p></fieldset>
    <fieldset class="sim-panel"><legend><span class="sim-step">02</span>Who should receive the ${bank?'patch':'memo'}?</legend><p class="sim-panel-help">Choose the actual audience for this version.</p><div class="sim-destinations">${scenario.audiences.sort((a,b)=>a.id===scenario.defaults.audience?-1:b.id===scenario.defaults.audience?1:0).map(a=>`<label class="sim-destination"><input type="radio" name="destination" value="${e(a.id)}" ${selection.audience===a.id?'checked':''}><span><strong>${e(a.title)}</strong><small>${e(a.description)}</small></span></label>`).join('')}</div><div id="sim-recipients"></div></fieldset></div>
    <section class="sim-panel sim-access"><div class="sim-section-heading"><div><h2>Can each recipient read the evidence?</h2><p>Every selected source carries its permissions into the ${bank?'patch':'memo'}.</p></div><span class="sim-live">${icon('shield',12)}Live check</span></div><div id="sim-matrix"></div></section>
    <section id="sim-next" class="sim-next"></section>
    <details class="sim-details"><summary>How the check works</summary><p>TrustGraph compiles the proposed reads, generation, approval and delivery before the agent acts. Every recipient must be permitted to read every selected source, including instructions. Removing a source starts a fresh generation context; editing an existing memo does not remove its history.</p><p>These are synthetic source permissions. A pass concerns authorized information flow; ${bank?'a reviewer still assesses the patch and tests.':'a partner still assesses the evidence and business recommendation.'} Generation and delivery use local simulators.</p><details><summary>Inspect the exact compiled plan</summary><pre id="sim-plan"></pre></details></details>
    <div class="sim-footer">${icon('lock',12)}No documents are changed or messages sent during simulation.<button class="sim-reset" data-connect>Connect agent →</button><a href="#/verification">Advanced verification →</a></div></div>`;
  const $=id=>host.querySelector(`#${id}`);
  const checks=()=>host.querySelectorAll('input[name="evidence"]');
  const updateControls=()=>{
    checks().forEach(c=>{c.checked=selection.sources.includes(c.value);c.closest('label').classList.toggle('selected',c.checked);});
    host.querySelectorAll('input[name="destination"]').forEach(c=>{c.checked=selection.audience===c.value;c.closest('label').classList.toggle('selected',c.checked);});
    const audience=scenario.audiences.find(a=>a.id===selection.audience);
    $('sim-recipients').innerHTML=`<div class="sim-recipient-caption">${audience.recipients.length} people will receive this version</div>${audience.recipients.map(id=>`<div class="sim-person"><span>${e(people[id]?.name.slice(0,1)||id.slice(0,1))}</span><div><strong>${e(people[id]?.name||id)}</strong><small>${e(people[id]?.role||'Recipient')}</small></div>${id===identity?'<em>You</em>':''}</div>`).join('')}`;
  };
  async function refresh(){
    updateControls();
    const request=++sequence;
    result=undefined;
    saved={identity,selection:structuredClone(selection)};
    $('sim-next').innerHTML='';
    $('sim-outcome').innerHTML='<div class="sim-verdict checking">Checking the proposed evidence and audience…</div>';
    if(!selection.sources.length){$('sim-outcome').innerHTML='<div class="sim-verdict checking"><strong>Select at least one evidence document</strong><p>The memo needs a declared source before it can be prepared.</p></div>';$('sim-matrix').innerHTML='';$('sim-plan').textContent='No plan: select an evidence document.';return;}
    try{
      const preview=await api('/scenario/simulate',selection);
      if(request!==sequence||version!==generation)return;
      result=preview;
      const safe=result.report.ready_to_generate;
      const conflict=result.conflicts[0];
      const audience=scenario.audiences.find(a=>a.id===selection.audience);
      $('sim-outcome').innerHTML=`<div class="sim-verdict ${safe?'allowed':'blocked'}"><span class="sim-verdict-icon">${icon(safe?'check':'lock',22)}</span><div><span class="sim-verdict-label">${safe?'PERMISSIONS PASS':'DELIVERY BLOCKED'}</span><strong>${safe?'This evidence can reach the selected audience.':conflict?`${e(people[conflict.recipient]?.name||conflict.recipient)} cannot receive work based on ${e(title(conflict.source))}.`:'This plan cannot proceed.'}</strong><p>${safe?(result.prepared_review?.status==='changes_requested'?'Source access passes. The prepared artifact still needs a revised agent plan.':result.prepared_review?.status==='released'?'This configuration already has an approved artifact and a delivery receipt.':`Prepare a fresh ${bank?'patch':'memo'}, then ask a ${bank?'reviewer':'partner'} to approve the exact content.`):conflict?'The agent can read this source. That permission does not extend to every recipient.':e(result.report.conflict?.message||'Check the declared inputs and destination.')}</p></div></div>`;
      $('sim-matrix').innerHTML=`<table class="sim-matrix"><caption class="sr-only">Source permissions for ${e(audience.title)}</caption><thead><tr><th scope="col">Evidence</th>${audience.recipients.map(id=>`<th scope="col">${e(people[id]?.name||id)}<small>${e(people[id]?.department||(id==='sam'?'Client':'Aster'))}</small></th>`).join('')}</tr></thead><tbody>${result.matrix.map(row=>`<tr class="${row.selected?'':'excluded'}"><th scope="row">${e(title(row.source))}${!row.selected?'<small>Not used in this artifact</small>':row.instruction?'<small>Always included</small>':''}</th>${row.cells.map(cell=>`<td><span class="permission-cell ${!row.selected?'unused':cell.allowed?'yes':'no'}">${icon(!row.selected?'info':cell.allowed?'check':'lock',14)}<span>${!row.selected?'Not used':cell.allowed?'Allowed':'Blocked'}</span></span></td>`).join('')}</tr>`).join('')}</tbody></table>`;
      if(safe&&result.prepared_review){
        const previous=result.prepared_review, returned=previous.status==='changes_requested', released=previous.status==='released';
        $('sim-next').innerHTML=`<div><span class="sim-next-label">${returned?'PARTNER REQUESTED CHANGES':released?'ALREADY DELIVERED':'ARTIFACT ALREADY PREPARED'}</span><h2>${returned?'Choose a revised plan before preparing again':released?'This version has a recorded delivery':'Continue with the existing partner review'}</h2><p>${returned?'Change the evidence set or audience to propose a different plan. Writing changes require the agent to submit a revised plan; this local renderer cannot apply prose feedback.':released?'This exact configuration was already approved and delivered to the local simulator. Open its review to inspect the receipt and evidence.':'The same inputs and audience reuse the prepared artifact. Opening it will not create another version or send a message.'}</p></div><a class="button primary" href="#/requests/${encodeURIComponent(previous.request)}">${icon(returned?'message':released?'shield':'files',15)}${returned?'Open returned review':released?'View delivered memo':'Open partner review'}${icon('arrow',14)}</a>`;
      }else if(safe){
        $('sim-next').innerHTML=`<div><span class="sim-next-label">NEXT STEP</span><h2>Prepare the ${bank?'patch':'decision memo'}</h2><p>A fresh context using ${selection.sources.length} ${selection.sources.length===1?'document':'documents'}, for ${e(audience.title.toLowerCase())}. ${bank?'Human':'Partner'} approval is still required.</p></div><button class="button primary" id="sim-prepare" ${scenario.can_prepare?'':'disabled'}>${icon('files',15)}Prepare ${bank?'patch':'memo'} for review${icon('arrow',14)}</button>${scenario.can_prepare?'':'<p class="sim-role-note">Switch to an authorized reviewer or agent to prepare work.</p>'}`;
        $('sim-prepare').onclick=async()=>{
          if(preparing||!result)return;
          preparing=true;const button=$('sim-prepare');button.disabled=true;button.textContent='Preparing tracked artifact…';
          // Freeze the preview being approved; controls cannot race this request.
          host.querySelectorAll('input, #sim-reset').forEach(el=>el.disabled=true);
          try{const run=await api('/scenario/prepare',{selection:structuredClone(selection),plan_digest:result.report.plan_digest,snapshot_digest:result.report.snapshot_digest});if(version!==generation)return;location.hash=`/requests/${encodeURIComponent(run.run.request)}`;await h.onUpdate();h.toast('Artifact prepared. A human must review this exact version.');}
          catch(error){if(version===generation){h.toast(error.message,true);host.querySelectorAll('input, #sim-reset').forEach(el=>el.disabled=false);await refresh();}}finally{preparing=false;}
        };
      }else if(conflict){
        const denied=new Set(result.conflicts.map(c=>c.source));
        // The alternate audience is selected from server-provided options.
        const alternate=scenario.audiences.find(a=>a.id!==selection.audience);
        $('sim-next').innerHTML=`<div class="sim-repair-title"><span class="sim-next-label">TWO WAYS FORWARD</span><h2>Change the plan, then test it again</h2></div><div class="sim-fixes"><button id="sim-fix-evidence"><span>${icon('files',19)}</span><div><strong>Keep the audience. Use shareable evidence.</strong><p>Remove ${[...denied].map(id=>e(title(id))).join(', ')}. Prepare a fresh ${bank?'patch':'memo'} from the remaining sources.</p></div>${icon('arrow',16)}</button>${alternate?`<button id="sim-fix-audience"><span>${icon('user',19)}</span><div><strong>Keep the evidence. Restrict the audience.</strong><p>Test delivery to ${e(alternate.title.toLowerCase())}. ${bank?'This will not prepare an Engineering deliverable.':'This will not prepare a client deliverable.'}</p></div>${icon('arrow',16)}</button>`:''}</div>`;
        $('sim-fix-evidence').onclick=()=>{selection.sources=selection.sources.filter(id=>!denied.has(id));refresh();};
        if($('sim-fix-audience'))$('sim-fix-audience').onclick=()=>{selection.audience=alternate.id;refresh();};
      }else $('sim-next').innerHTML='<p>Review the compiler diagnostic above before continuing.</p>';
      $('sim-plan').textContent=JSON.stringify({plan:result.plan,analysis:result.report},null,2);
    }catch(error){if(request!==sequence||version!==generation)return;$('sim-outcome').innerHTML=`<div class="sim-verdict blocked"><strong>Unable to verify this selection</strong><p>${e(error.message)}</p></div>`;$('sim-matrix').innerHTML='';$('sim-plan').textContent='No verified plan.';}
  }
  checks().forEach(c=>c.onchange=()=>{selection.sources=[...checks()].filter(c=>c.checked).map(c=>c.value);refresh();});
  host.querySelectorAll('input[name="destination"]').forEach(c=>c.onchange=()=>{selection.audience=c.value;refresh();});
  $('sim-reset').onclick=()=>{selection=structuredClone(scenario.defaults);refresh();};
  await refresh();
}
