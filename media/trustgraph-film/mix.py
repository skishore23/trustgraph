#!/usr/bin/env python3
"""Conform the original Comfy narration, score and exact vector picture timeline."""
from pathlib import Path
import json, re, subprocess
root=Path(__file__).resolve().parent
timeline=json.loads((root/'timeline.json').read_text())
args=['ffmpeg','-y','-hide_banner']
for s in timeline['scenes']:args+=['-i',s['file']]
filters=[]
for i,s in enumerate(timeline['scenes']):
    filters.append(f'[{i}:a]aresample=48000,aformat=channel_layouts=stereo,adelay={round(s["voice_start"]*1000)}|{round(s["voice_start"]*1000)}[v{i}]')
filters.append(''.join(f'[v{i}]' for i in range(8))+f'amix=inputs=8:normalize=0,apad,atrim=duration={timeline["duration"]}[vo]')
subprocess.run(args+['-filter_complex',';'.join(filters),'-map','[vo]','-c:a','pcm_s24le',str(root/'audio'/'narration.wav')],check=True,stdout=subprocess.DEVNULL,stderr=(root/'audio'/'narration.log').open('w'))
# Narration controls gentle score ducking. Normalize the resulting stereo master.
filters='[0:a]asplit=2[voice][control];[1:a]volume=0.62[music];[music][control]sidechaincompress=threshold=0.025:ratio=4:attack=20:release=450[duck];[voice][duck]amix=inputs=2:normalize=0,loudnorm=I=-14:TP=-1.5:LRA=7[mix]'
subprocess.run(['ffmpeg','-y','-hide_banner','-i',str(root/'audio'/'narration.wav'),'-i',str(root/'audio'/'score.wav'),'-filter_complex',filters,'-map','[mix]','-ar','48000','-c:a','pcm_s24le',str(root/'audio'/'master.wav')],check=True,stdout=subprocess.DEVNULL,stderr=(root/'audio'/'mix.log').open('w'))
def timecode(t):
    ms=round(t*1000);return f'{ms//3600000:02}:{ms//60000%60:02}:{ms//1000%60:02},{ms%1000:03}'
subs=[]
for s in timeline['scenes']:
    sentences=re.split(r'(?<=[.!?])\s+',s['text'])
    total=sum(len(x.split()) for x in sentences);cursor=s['voice_start']
    for sentence in sentences:
        duration=s['voice']*len(sentence.split())/total
        words=sentence.split();lines=[];line=''
        for word in words:
            if len(line)+len(word)>64:lines.append(line);line=''
            line+=((' ' if line else '')+word)
        if line:lines.append(line)
        subs.append(f'{len(subs)+1}\n{timecode(cursor)} --> {timecode(cursor+duration)}\n'+ '\n'.join(lines)+'\n')
        cursor+=duration
srt=root/'deliverables'/'TrustGraph.srt';srt.write_text('\n'.join(subs))
subprocess.run(['ffmpeg','-y','-hide_banner','-i',str(root/'outputs'/'picture.mp4'),'-i',str(root/'audio'/'master.wav'),'-i',str(srt),'-map','0:v','-map','1:a','-map','2:s','-c:v','copy','-c:a','aac','-b:a','256k','-c:s','mov_text','-metadata','title=TrustGraph - Permission follows the work','-metadata','comment=Original Comfy illustrations and synthetic narration; original JavaScript motion design and score. Controlled bank case.','-t',str(timeline['duration']),'-movflags','+faststart',str(root/'deliverables'/'TrustGraph-Architecture.mp4')],check=True,stdout=subprocess.DEVNULL,stderr=(root/'audio'/'mux.log').open('w'))
print('Final film:',root/'deliverables'/'TrustGraph-Architecture.mp4')
