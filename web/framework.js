const $ = selector => document.querySelector(selector);
let scenario = 'documents';
let latest = 0;
let current = null;
const ns = 'http://www.w3.org/2000/svg';
const makeSvg = (tag, attrs = {}) => { const el = document.createElementNS(ns, tag); for (const [key, value] of Object.entries(attrs)) el.setAttribute(key, String(value)); return el; };
const label = (value, max = 24) => String(value ?? '').length > max ? String(value).slice(0, max - 1) + '…' : String(value ?? '');
const statusNames = {allow:'Allowed',needs_approval:'Approval required',deny:'Blocked',invalid:'Execution rejected'};
const statusClasses = {allow:'allow',needs_approval:'approval',deny:'deny',invalid:'deny'};

function inspect(node, group) {
  for (const el of document.querySelectorAll('.svg-node.selected')) el.classList.remove('selected');
  group?.classList.add('selected');
  $('#selected-help').textContent = `${node.label} · ${node.kind}`;
  const records = current?.records;
  const detail = records?.artifacts?.[node.id] || records?.executions?.[node.id] || records?.operations?.[node.id];
  $('#record').textContent = JSON.stringify(detail || node, null, 2);
}
function drawGraph(data) {
  const target = $('#graph'); target.replaceChildren();
  if (!Array.isArray(data.nodes) || !data.nodes.length) { const p = document.createElement('p'); p.className='empty'; p.textContent='No outputs were admitted. The framework rejected this execution.'; target.append(p); return; }
  const nodes = data.nodes, edges = data.edges || [], ids = new Set(nodes.map(n=>n.id));
  const incoming = new Map(nodes.map(n=>[n.id, []]));
  for (const edge of edges) if (ids.has(edge.from) && ids.has(edge.to)) incoming.get(edge.to).push(edge.from);
  const layers = new Map();
  function layer(id, visiting = new Set()) { if(layers.has(id)) return layers.get(id); if(visiting.has(id)) return 0; const seen=new Set(visiting); seen.add(id); const parents=incoming.get(id)||[]; const value=parents.length ? 1 + Math.max(...parents.map(p=>layer(p,seen))) : 0; layers.set(id,value); return value; }
  for(const node of nodes) layer(node.id);
  const columns = new Map(); for(const node of nodes) { const l=layers.get(node.id); if(!columns.has(l)) columns.set(l,[]); columns.get(l).push(node); }
  const cols=Math.max(...layers.values())+1, rows=Math.max(...[...columns.values()].map(v=>v.length));
  const width=Math.max(760, cols*185+30), height=Math.max(340,rows*112+34);
  const svg=makeSvg('svg',{width,height,viewBox:`0 0 ${width} ${height}`,role:'group','aria-label':'Recorded dependency graph. Select a node to inspect its record.'});
  svg.classList.add('dataflow-svg');
  const defs=makeSvg('defs');const marker=makeSvg('marker',{id:'arrow',markerWidth:7,markerHeight:7,refX:6,refY:3.5,orient:'auto'});marker.append(makeSvg('path',{d:'M0,0 L7,3.5 L0,7 z',fill:'#69815f'}));defs.append(marker);svg.append(defs);
  const pos=new Map(); for(const [col, list] of columns) list.forEach((node,row)=>pos.set(node.id,{x:20+col*185,y:(height-list.length*112)/2+row*112}));
  const affected=new Set(data.affected||[]);
  for(const edge of edges){const from=pos.get(edge.from),to=pos.get(edge.to);if(!from||!to)continue;const x=from.x+145,y=from.y+43,tx=to.x,ty=to.y+43;const path=makeSvg('path',{d:`M ${x} ${y} C ${x+22} ${y}, ${tx-22} ${ty}, ${tx} ${ty}`,'marker-end':'url(#arrow)'});path.classList.add('edge');if(affected.has(edge.from)&&affected.has(edge.to))path.classList.add('affected');svg.append(path);const portLabel=makeSvg('text',{x:(x+tx)/2,y:(y+ty)/2-7,'text-anchor':'middle'});portLabel.classList.add('edge-label');portLabel.textContent=label(edge.label,14);svg.append(portLabel);}
  for(const node of nodes){const p=pos.get(node.id);const operation=/operation|execution|transform/i.test(node.kind);const group=makeSvg('g',{transform:`translate(${p.x} ${p.y})`,tabindex:0,role:'button','aria-label':`Inspect ${node.label}, ${node.kind}, type ${node.schema||'operation'}`});group.classList.add('svg-node');if(operation)group.classList.add('operation');if(affected.has(node.id))group.classList.add('affected');group.append(makeSvg('rect',{width:145,height:88,rx:operation?14:6}));const title=makeSvg('title');title.textContent=node.label;group.append(title);
    const add=(text,y,cls)=>{const el=makeSvg('text',{x:11,y});el.classList.add(cls);el.textContent=text;group.append(el);};
    add(label(node.kind.toUpperCase(),24),18,'svg-kind');add(label(node.label,22),37,'svg-title');add(label(node.schema||'typed operation',25),55,'svg-schema');add(node.readers?.length?label(node.readers.join(' · '),29):operation?'named inputs → outputs':'no permitted readers',74,'svg-readers');
    group.addEventListener('click',()=>inspect(node,group));group.addEventListener('keydown',event=>{if(event.key==='Enter'||event.key===' '){event.preventDefault();inspect(node,group);}});svg.append(group);
  }
  target.append(svg);
}
function render(data) {
  current=data; drawGraph(data);
  const box=$('#decision');box.className=`decision ${statusClasses[data.status]||'error'}`;
  box.querySelector('.decision-title').textContent=statusNames[data.status]||'Unexpected response';
  box.querySelector('p').textContent=data.reason || 'Inspect the returned evidence below.';
  $('#events').replaceChildren();
  for(const step of data.steps||[]) {const li=document.createElement('li');li.textContent=step;$('#events').append(li);}
  if(data.before_status&&data.before_status!==data.status){const li=document.createElement('li');li.textContent=`Before change: ${statusNames[data.before_status]||data.before_status}. After change: ${statusNames[data.status]||data.status}.`;$('#events').append(li);}
  $('#selected-help').textContent='Select a node above for its full record. Current decision:';
  $('#record').textContent=JSON.stringify(data.decision||{status:data.status,reason:data.reason,phase:data.phase},null,2);
  $('#engine-status').textContent='● VERIFIED BY RUST';$('#engine-status').className='engine-status ready';
}
async function evaluate(){
  const requestId=++latest;$('#run').disabled=true;$('#engine-status').textContent='EVALUATING…';$('#engine-status').className='engine-status';
  const body={scenario,audience:$('#recipient').value,approve:$('#approved').checked,mutation:$('#change').value};
  try{const response=await fetch('/api/framework',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)});const data=await response.json();if(requestId!==latest)return;if(!response.ok)throw new Error(data.error||`Server returned ${response.status}`);if(!['allow','needs_approval','deny','invalid'].includes(data.status))throw new Error('The server did not return a framework decision.');render(data);}
  catch(error){if(requestId!==latest)return;const box=$('#decision');box.className='decision error';box.querySelector('.decision-title').textContent='Could not verify';box.querySelector('p').textContent=error.message;$('#engine-status').textContent='ENGINE UNAVAILABLE';$('#engine-status').className='engine-status';$('#graph').replaceChildren();const p=document.createElement('p');p.className='empty';p.textContent='No current evidence. Retry when the server is available.';$('#graph').append(p);$('#events').replaceChildren();$('#record').textContent='No verified result.';current=null;}
  finally{if(requestId===latest)$('#run').disabled=false;}
}
function reset(){ $('#recipient').value='finance';$('#approved').checked=false;$('#change').value='none'; }
for(const button of document.querySelectorAll('.scenario'))button.addEventListener('click',()=>{scenario=button.dataset.scenario;for(const el of document.querySelectorAll('.scenario'))el.classList.toggle('active',el===button);reset();$('#approved').disabled=scenario==='export';$('#approval-help').textContent=scenario==='export'?'This export rule does not require approval.':'A fictional reviewer approves the initial version.';evaluate();});
$('#run').addEventListener('click',evaluate);
$('#reset').addEventListener('click',()=>{reset();evaluate();});
for(const id of ['recipient','approved','change'])$('#'+id).addEventListener('change',evaluate);
evaluate();
