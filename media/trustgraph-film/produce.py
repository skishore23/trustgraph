#!/usr/bin/env python3
"""Produce original TrustGraph film assets with the authenticated comfy CLI."""
from pathlib import Path
import argparse, datetime, json, re, shutil, subprocess, sys
ROOT = Path(__file__).resolve().parent

def log(message):
    with (ROOT/'LOG.md').open('a') as f:
        f.write(datetime.datetime.now(datetime.timezone.utc).isoformat()+' '+message+'\n')

def comfy(args):
    result = subprocess.run(['comfy','--json',*args],capture_output=True,text=True,cwd=ROOT)
    values = []
    for line in result.stdout.splitlines():
        try: values.append(json.loads(line))
        except json.JSONDecodeError: pass
    envelope = next((v for v in reversed(values) if v.get('type')=='envelope'),None)
    if envelope is None:
        raise RuntimeError('comfy returned no envelope: '+result.stderr[-500:])
    if not envelope.get('ok'):
        log('ERROR '+json.dumps(envelope.get('error')))
        raise RuntimeError(json.dumps(envelope.get('error')))
    return envelope

def variant(template, name, slots):
    path=ROOT/'workflows'/(name+'.json')
    shutil.copyfile(ROOT/'workflows'/(template+'.json'),path)
    comfy(['workflow','set-slot',str(path),*[k+'='+json.dumps(v) for k,v in slots.items()]])
    return path

def submit(name, path):
    response = comfy(['run','--workflow',str(path),'--allow-spend'])
    (ROOT/'reference'/('submit-'+name+'.json')).write_text(json.dumps(response,indent=2))
    data=response['data']; identifier=data['prompt_id']
    with (ROOT/'job_ids.txt').open('a') as f:f.write(name+' '+identifier+'\n')
    log('SUBMIT '+name+' '+identifier)
    print(name,identifier,flush=True)
    return response

ART_STYLE='Original editorial cut-paper collage illustration, hand-drawn charcoal ink contour, subtle pencil hatching and fine warm paper grain, slightly imperfect human geometry, thoughtful sophisticated educational film, restrained ivory #f4f0e5, charcoal #202722, desaturated forest green #456a57, mint, vermilion and ochre accents, coherent gentle light from upper left. Paper layers with soft physical shadows. Flat side-on graphic composition with depth from paper layers, not 3D CGI. No letters, no words, no numbers, no logos, no watermark. Do not copy any existing artwork.'
ART={
'paper_world':ART_STYLE+' Wide panoramic paper city for a software governance story. At lower left a charming small cut-paper coding robot seated at an ink-drawn desk with an unlabelled monitor. In the lower right a large hand-crafted hinged gateway with two simple blank paper posts. A narrow bridge of paper leads between them. A few scattered blank source documents. The upper sixty percent is generous empty ivory paper for animated typography. Quiet, witty, beautiful composition; no floating circuit boards, no generic futuristic blue glow.',
'bank_world':ART_STYLE+' Wide editorial tableau: a little hand-drawn bank at lower center, a green engineering workshop on the lower left and a confidential coral paper archive on the lower right, separated by visible ivory negative space. Tiny people made of cut paper are handling documents. A small bridge leads out of the workshop. No readable signs or symbols in the buildings. The upper half remains empty ivory paper for clean diagram labels. Clear visual metaphors for separate audiences, lively but restrained, consistent print illustration style.',
'prop_sheet':ART_STYLE+' Create a precisely aligned 3-column by 2-row contact sheet of six isolated cut-paper props, each fully inside its own equal cell with generous ivory background. Top row left: a friendly paper coding robot at a tiny desk; top middle: a human reviewer with round glasses holding a blank checklist; top right: a stack of three paper database cylinders connected by little ink threads. Bottom row left: two blank paper source documents, one forest green and one coral; bottom middle: a little hinged release gateway with a paper padlock; bottom right: a small hand-drawn bank building with simple pillars. No prop crosses a cell boundary. Consistent scale, human handmade illustration style, ivory paper background with very subtle grain, no frames around the cells.'
}

def prepare_art():
    for i,(name,prompt) in enumerate(ART.items()):
        path=variant('api_nano_banana_2_1_t2i',name,{'1.prompt':prompt,'1.model.aspect_ratio':'16:9' if name!='prop_sheet' else '3:2','1.model.resolution':'2K','1.model.thinking_level':'HIGH','1.seed':7310+i,'3.filename_prefix':'trustgraph-film/'+name})
        submit(name,path)

def prepare_voice():
    script=(ROOT/'SCRIPT.md').read_text()
    for i,text in enumerate(re.findall(r'### [1-8]\n(.*?)(?=\n### |\n## Sound)',script,re.S),1):
        text=' '.join(text.split())
        name='voice_'+str(i)
        path=variant('api_elevenlabs_text_to_speech',name,{'208.text':text,'208.stability':0.62,'208.model.speed':1.02,'208.model.similarity_boost':0.78,'208.model.use_speaker_boost':True,'208.model.style':0.12,'208.language_code':'en','208.seed':481,'211.voice':'Roger (male, american)','218.filename_prefix':'trustgraph-film/'+name,'218.format':'flac'})
        submit(name,path)

def watch_all():
    jobs=[line.split() for line in (ROOT/'job_ids.txt').read_text().splitlines() if line.strip()]
    for name,identifier in jobs:
        marker=ROOT/'reference'/('completed-'+name+'-'+identifier+'.json')
        if marker.exists():continue
        failure = ROOT/'reference'/('failed-'+name+'-'+identifier+'.json')
        if failure.exists():continue
        try:
            response=comfy(['jobs','watch',identifier,'--max-wait','1800']);marker.write_text(json.dumps(response,indent=2));log('COMPLETE '+name+' '+identifier)
            downloaded=comfy(['download',identifier,'--out-dir',str(ROOT/'outputs'/name)])
            (ROOT/'reference'/('download-'+name+'.json')).write_text(json.dumps(downloaded,indent=2))
            log('DOWNLOADED '+name);print('ready',name,flush=True)
        except Exception as error:
            pending = 'cloud_timeout' in str(error)
            marker_name = ('pending-' if pending else 'failed-')+name+'-'+identifier+'.json'
            (ROOT/'reference'/marker_name).write_text(json.dumps({'error':str(error),'name':name,'prompt_id':identifier},indent=2))
            log(('WATCH_PENDING ' if pending else 'FAILED ')+name+' '+identifier+' '+str(error)[:300])
            print('FAILED',name,str(error)[:180],flush=True)

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('action',choices=['art','voice','watch']);args=parser.parse_args()
    {'art':prepare_art,'voice':prepare_voice,'watch':watch_all}[args.action]()
