#!/usr/bin/env python3
"""Source13 actual Files sorting/filtering/content GUI gate on an owned disk.

No image building. Actual keyboard/pointer inputs, exact source-bound pixels,
read-only real Store oracle, and protected-prefix hashes are required for PASS.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
import boot_smoke
import files_query_host as oracle
import prism_files_appearance_smoke as base

ROOT=base.ROOT
A='/native/query-A'
Z='/native/query-z'
M='/native/query-middle'
H='/native/query-hidden'
LONG=b'a'*180+b'needle'


def exercise(args,image,helper,ui_helper,original,serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report):
    original(serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report)
    width,height=map(int,wait_regex(rb'PRISM: READY (\d+)x(\d+);').groups())
    geometry={name:tuple(map(int,values)) for name,*values in
              (line.split() for line in subprocess.check_output([str(ui_helper),'geometry',str(width),str(height)]).decode().splitlines())}
    output=args.output_dir/'query';output.mkdir(parents=True,exist_ok=True)
    actions=[];captures=[];results=[]
    def frame(mark):return wait_regex(rb'PRISM (?:OUTPUT )?FRAME: generation=\d+',mark)
    def key(*codes):
        mark=len(data);qmp('send-key',{'keys':[{'type':'qcode','data':c} for c in codes],'hold-time':40})
        actions.append({'keys':codes,'serial_start':mark});time.sleep(min(.075,remaining()));return mark
    def click_bounds(bounds):
        target=[bounds[0]+bounds[2]//2,bounds[1]+bounds[3]//2]
        def feedback(delta):
            mark=len(data);events=[{'type':'rel','data':{'axis':axis,'value':delta[i]}} for i,axis in enumerate(('x','y')) if delta[i]]
            events.append({'type':'btn','data':{'button':'middle','down':True}})
            qmp('input-send-event',{'events':events});event=wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=4',mark);frame(event.end())
            release=len(data);qmp('input-send-event',{'events':[{'type':'btn','data':{'button':'middle','down':False}}]})
            end=wait_regex(rb'PRISM POINTER: [^\r\n]*buttons=0[^\r\n]*',release);frame(end.end())
            return list(map(int,event.groups()))
        pos=feedback([0,0])
        for _ in range(64):
            if pos==target:break
            pos=feedback([max(-90,min(90,target[i]-pos[i])) for i in range(2)])
        else:raise RuntimeError('actual pointer failed to converge')
        mark=len(data);qmp('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':True}}]})
        event=wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=1',mark)
        if list(map(int,event.groups()))!=target:raise RuntimeError('actual pointer missed source-bound control')
        release=len(data);qmp('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':False}}]})
        end=wait_regex(rb'PRISM POINTER: [^\r\n]*buttons=0[^\r\n]*',release);frame(end.end())
        actions.append({'click':target,'serial_start':mark});return mark
    def click(name):return click_bounds(geometry[name])
    def command(text,expected):
        click('terminal_button');mark=len(data);serial.sendall((text+'\r').encode())
        event=wait_text(expected,mark);frame(event.end() if hasattr(event,'end') else data.index(expected.encode(),mark)+len(expected));return mark
    def journal():
        qmp('stop')
        try:return base.journal_state(image,args.store_start)
        finally:qmp('cont')
    def checked_query(text='',sort='name',descending=False,content=False):
        qmp('stop')
        try:
            try:return oracle.query(helper,image,args.store_start,width,height,text,sort,descending,content)
            except subprocess.CalledProcessError as error:
                raise RuntimeError('readonly actual Store query helper rejected guest journal') from error
        finally:qmp('cont')
    def screenshot(name):
        path=output/(name+'.ppm');qmp('screendump',{'filename':str(path),'format':'ppm'})
        captures.append({'path':str(path.relative_to(ROOT)),'sha256':base.digest(path)});return base.ppm(path.read_bytes())
    def text_match(shown,x,y,text):
        for selected in ('surface','selected'):
            template=base.ppm(subprocess.check_output([str(helper),'text',selected,text]))
            actual=base.crop(shown,(x,y,template[0],template[1]))
            bg=template[2][-3:]
            foreground=[i for i in range(0,len(actual),3) if template[2][i:i+3]!=bg]
            if foreground and all(actual[i:i+3]==template[2][i:i+3] for i in foreground):return True
        return False
    current={'text':'','sort':'name','descending':False,'content':False}
    desired=current.copy()
    def assert_rows(label,expected):
        shown=screenshot(label)
        for row in expected['rows']:
            if not text_match(shown,row['x'],row['text_y'],row['path']):
                raise RuntimeError(f'actual {label} row {row["position"]} did not render checked path {row["path"]}')
        fx,fy,_,_=geometry['files']
        if not text_match(shown,fx+12,fy+112,'/NEXT.TXT'):
            raise RuntimeError('filtered Files lost separate boot document')
        _,_,_,fh=geometry['files']
        summary=f"{expected['matched']} of {expected['total']} stored files / boot document separate"
        if not text_match(shown,fx+12,fy+fh-68,summary):
            raise RuntimeError('actual Files query count summary did not match checked result')
        # A zero-result filter must retire the prior native first row, not just
        # update a counter while retaining stale file text.
        if not expected['rows']:
            for prior in checked_query()['rows']:
                if text_match(shown,fx+12,fy+136,prior['path']):
                    raise RuntimeError('empty query retained a stale native row')
        results.append({'label':label,**expected})
    def apply(text='',sort='name',descending=False,content=False,cancel=False):
        before=journal();expected=checked_query(text,sort,descending,content)
        click('files_button');key('slash');redraw=key('right');frame(redraw)
        controls=expected['controls'];click_bounds(controls[0])
        for _ in desired['text']:key('backspace')
        for char in text:key({'-':'minus','.':'dot','/':'slash',' ':'spc'}.get(char,char))
        if sort!=desired['sort']:click_bounds(controls[1])
        if descending!=desired['descending']:click_bounds(controls[2])
        if content!=desired['content']:click_bounds(controls[3])
        desired.update(text=text,sort=sort,descending=descending,content=content)
        mark=click_bounds(controls[4 if cancel else 5])
        if not cancel:
            event=wait_regex(rb'PRISM FILE QUERY: request=(\d+) sort=([^ ]+) descending=(true|false) search=([^ ]+) result=Ok\(Stats \{ revision: (\d+), total: (\d+), matched: (\d+) \}\)',mark)
            got=(int(event[5]),int(event[6]),int(event[7]))
            if got!=(expected['revision'],expected['total'],expected['matched']):raise RuntimeError('actual query callback disagrees with readonly real Store oracle')
            if event[2].decode()!=sort.title() or (event[3]==b'true')!=descending or event[4].decode()!=('PathAndContent' if content else 'Path'):
                raise RuntimeError('actual query input differed from requested controls')
            frame(event.end());current.update(text=text,sort=sort,descending=descending,content=content)
        else:
            if b'PRISM FILE QUERY:' in data[mark:]:raise RuntimeError('Cancel dispatched an actual query')
        if journal()!=before:raise RuntimeError('readonly query changed journal bytes or generation')
        assert_rows(('cancel' if cancel else 'apply')+'-'+str(len(results)),checked_query(**current))
        return expected
    # Seed through trusted console; make the >128-byte content via actual Editor.
    for path,text in [(A,'x'),(M,'medium-size-twentyxx'),(Z,'seed'),(H,'needle-private')]:
        command('write '+path+' '+text,'NEXT DATA: Wrote '+str(len(text))+' bytes')
    command('edit '+Z,'PRISM EDITOR OPEN: path='+Z+' bytes=4 exists=true')
    key('end')
    for _ in range(4):key('backspace')
    for char in LONG.decode():key(char)
    mark=key('ctrl','s');saved=wait_regex(rb'PRISM EDITOR SAVE: path='+re.escape(Z.encode())+rb' bytes=186 revision=\d+',mark);frame(saved.end())
    if journal()[1][Z]['data']!=LONG:raise RuntimeError('actual Editor did not commit full search fixture')
    apply('query-',sort='name');apply('query-',sort='size');apply('query-',sort='size',descending=True)
    apply('query-a',sort='name');apply('never-matches',cancel=True)
    # Move a real matching payload to private Trash using actual controls.
    default=apply();hidden=next(row for row in default['rows'] if row['path']==H)
    click_bounds((hidden['x'],hidden['text_y']-6,300,24));mark=click('file_delete')
    # Source13 Trash still defaults Cancel; explicitly Tab to confirm.
    key('tab');mark=key('ret');event=wait_regex(rb'PRISM TRASH COMMIT: request=\d+ intent=Move[^\r\n]*result=Ok\(\(\)\)',mark);frame(event.end())
    hidden_result=apply('needle',content=True)
    if [r['path'] for r in hidden_result['rows']]!=[Z]:raise RuntimeError('private Trash data leaked into content query')
    # Keep the filtered view active while the actual chooser uses full bindings.
    click('terminal_button');original(serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report)
    report['checks'].append('actual portal consent still exposes existing nonmatching paths while Files content filter is active')
    click('settings_button');mark=click('appearance_2_0')
    event=wait_regex(rb'PRISM APPEARANCE SAVE: request=\d+ theme=Light accent=Blue result=Ok\(\(\)\) accepted=true',mark);frame(event.end())
    if len(journal()[1].get('/prefs/appearance',{}).get('data',b''))!=32:raise RuntimeError('real binary preference fixture missing')
    prefs=apply('nxapref',content=True)
    if prefs['matched']!=0:raise RuntimeError('private preference bytes participated in content search')
    path_only=apply('needle',content=False)
    if path_only['matched']!=0:raise RuntimeError('path-only query silently searched file content')
    full=apply('needle',content=True)
    if [r['path'] for r in full['rows']]!=[Z]:raise RuntimeError('full content beyond compact preview was not found')
    apply();click('terminal_button')
    report['files_query']={'actions':actions,'captures':captures,'results':results,'full_content_bytes':len(LONG),'scope':'Bounded ASCII single-profile Files-only query; no index/Unicode/latency claim.'}
    report['checks'].append('actual Files query keyboard/pointer controls render checked Name/Size/direction/filter results; Cancel and every query preserve durable generation/bytes; full content beyond128-byte preview matches, Trash/prefs content excluded')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image',type=Path,default=ROOT/'work/thirteenth-wave/frozen-src/build/next-dev.img')
    parser.add_argument('--source-manifest',type=Path,default=ROOT/'work/thirteenth-wave/source-manifest.json')
    parser.add_argument('--frozen-source-root',type=Path,default=ROOT/'work/thirteenth-wave/frozen-src')
    parser.add_argument('--output-dir',type=Path,default=ROOT/'build/files-query-smoke')
    parser.add_argument('--timeout',type=float,default=450)
    parser.add_argument('--store-start',type=int,default=131072)
    args=parser.parse_args();args.output_dir=args.output_dir.resolve();args.image=args.image.resolve()
    if not args.output_dir.is_relative_to(ROOT/'build'):parser.error('output must be beneath build/')
    expected=json.loads((ROOT/'work/thirteenth-wave/prepare.json').read_text())['sha256']['next-dev.img']
    if base.digest(args.image)!=expected:parser.error('image differs from immutable prepared Source13 identity; no VM launched')
    args.output_dir.mkdir(parents=True,exist_ok=True)
    helper,binding=oracle.ensure_host(args.source_manifest.resolve(),args.frozen_source_root.resolve(),args.output_dir/'query-host')
    ui_helper,ui_binding=base.ensure_host(args.source_manifest.resolve(),args.frozen_source_root.resolve(),args.output_dir/'ui-host')
    original=boot_smoke.exercise_file_portal;argv=sys.argv[:];initial=base.digest(args.image)
    report={'passed':False,'image_sha256':initial,'query_binding':binding,'ui_binding':ui_binding}
    try:
        with tempfile.TemporaryDirectory(prefix='query-thirteen-',dir=ROOT/'work') as directory:
            scratch=Path(directory)/'scratch.img';shutil.copyfile(args.image,scratch);protected=base.protected_digest(scratch,args.store_start)
            boot_smoke.exercise_file_portal=lambda *hook:exercise(args,scratch,helper,ui_helper,original,*hook)
            sys.argv=[str(ROOT/'tools/boot_smoke.py'),'--image',str(scratch),'--output-dir',str(args.output_dir/'boot'),'--timeout',str(args.timeout),'--persistent-data']
            status=boot_smoke.main();boot=json.loads((args.output_dir/'boot/boot-smoke.json').read_text())
            report['protected_bytes_unchanged']=base.protected_digest(scratch,args.store_start)==protected
            report['passed']=status==0 and boot.get('passed') is True and report['protected_bytes_unchanged']
            report['boot_checks']=len(boot.get('checks',[]));report['error']=boot.get('error')
            if not report['passed']:
                failed=args.output_dir/'failed-scratch.img'
                shutil.copyfile(scratch,failed)
                report['failed_scratch_sha256']=base.digest(failed)
    finally:
        boot_smoke.exercise_file_portal=original;sys.argv=argv
        report['input_image_unchanged']=base.digest(args.image)==initial
        report['helper_source_unchanged']=oracle.digest(oracle.HOST)==binding['helper_source_sha256']
        report['source_manifest_unchanged']=oracle.digest(args.source_manifest)==binding['manifest_sha256']
        if not all(report.get(k) for k in ('input_image_unchanged','helper_source_unchanged','source_manifest_unchanged')):report['passed']=False
        (args.output_dir/'files-query-smoke.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2));return 0 if report['passed'] else 1

if __name__=='__main__':sys.exit(main())
