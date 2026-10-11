#!/usr/bin/env python3
"""Source-bound image10 GUI Copy and Super/AltF4 extension; real guest only.

Reuses the two-boot Files/Appearance gate without mutating its ninth evidence.
Provide an immutable image and exact retained source manifest explicitly. This
script has host syntax/model checks only until the guest run succeeds.
"""
import re
import subprocess
import sys
import time
from pathlib import Path
import prism_files_appearance_smoke as base

ROOT=base.ROOT
HOST_SOURCE=ROOT/'tools/prism_shortcuts_copy_host.rs'
original_exercise=base.exercise

def exercise(args,phase,image,helper,binding,original,serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report):
    original_exercise(args,phase,image,helper,binding,original,serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report)
    if phase!='create':return
    dimensions=wait_regex(rb'PRISM: READY (\d+)x(\d+);')
    width,height=map(int,dimensions.groups())
    geometry={name:tuple(map(int,values)) for name,*values in (line.split() for line in subprocess.check_output([str(helper),'geometry',str(width),str(height)]).decode().splitlines())}
    pointers=list(re.finditer(rb'PRISM POINTER: x=(\d+) y=(\d+)',data))
    pointer=list(map(int,pointers[-1].groups()))
    output=args.output_dir/phase/'shortcuts-copy';output.mkdir(parents=True,exist_ok=True)
    captures=[];timeline=[]
    def key(*codes,frame=True):
        mark=len(data);timeline.append({'keys':codes,'serial_start':mark})
        qmp('send-key',{'keys':[{'type':'qcode','data':code} for code in codes],'hold-time':40})
        if frame:wait_text('PRISM FRAME:',mark)
        time.sleep(min(.07,remaining()));return mark
    def click(name):
        b=geometry[name] if isinstance(name,str) else name
        target=[b[0]+b[2]//2,b[1]+b[3]//2]
        while pointer!=target:
            delta=[max(-90,min(90,target[i]-pointer[i])) for i in range(2)]
            qmp('input-send-event',{'events':[{'type':'rel','data':{'axis':axis,'value':delta[i]}} for i,axis in enumerate(('x','y')) if delta[i]]})
            pointer[:]=[pointer[i]+delta[i] for i in range(2)];time.sleep(min(.02,remaining()))
        mark=len(data);timeline.append({'click':target,'serial_start':mark})
        qmp('input-send-event',{'events':[{'type':'btn','data':{'down':True,'button':'left'}}]})
        found=wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=1',mark)
        if list(map(int,found.groups()))!=target:raise RuntimeError('actual pointer differs from source-bound action')
        released=len(data)
        qmp('input-send-event',{'events':[{'type':'btn','data':{'down':False,'button':'left'}}]})
        event=wait_regex(rb'PRISM POINTER: [^\r\n]*buttons=0[^\r\n]*',released)
        wait_text('PRISM FRAME:',event.end());return mark
    def capture(name):
        path=output/(name+'.ppm');qmp('screendump',{'filename':str(path),'format':'ppm'})
        captures.append({'name':name,'sha256':base.digest(path),'path':str(path.relative_to(ROOT))})
        return base.ppm(path.read_bytes())
    def command(text,expected):
        click('terminal_button');mark=len(data);serial.sendall((text+'\r').encode())
        event=wait_regex(re.escape(expected.encode()),mark);wait_text('PRISM FRAME:',event.end());return mark
    def disk():
        qmp('stop')
        try:return base.journal_state(image,args.store_start)[1]
        finally:qmp('cont')
    def select(path):
        files=disk()
        if path not in files:raise RuntimeError('selection not in authoritative committed listing')
        click('files_button');return click('file_row_'+str(files[path]['gui_row']))
    def type_text(text):
        for char in text:key({'/':'slash','-':'minus','.':'dot',' ':'spc'}.get(char,char),frame=False)
        key('right')
    def replace_path(old,new):
        click('dialog_input')
        for _ in old:key('backspace',frame=False)
        type_text(new)
    def desktop(app,mark,home=False):
        event=wait_regex(re.escape(('PRISM DESKTOP: focused=Some('+app+') home='+str(home).lower()).encode())+rb'[^\r\n]*',mark)
        wait_text('PRISM FRAME:',event.end());return event
    def assert_bytes(path,expected):
        if disk().get(path,{}).get('data')!=expected:raise RuntimeError('committed bytes disagree for '+path)
    # Open actual windows with shortcuts and verify source-bound titles/present.
    for chord,app,title,name in [('i','Settings','Next Settings','settings'),('e','Files','Prism Files','files'),('ret','Terminal','Next Terminal','terminal')]:
        mark=key('meta_l',chord);desktop(app,mark)
        shown=capture('super-open-'+app.lower())
        if name in geometry:
            x,y,_,_=geometry[name]
            if not base.text_matches(helper,shown,x+42,y+10,title,'dark','violet'):raise RuntimeError('shortcut did not render '+title)
    mark=key('meta_l');desktop('Terminal',mark,True)
    capture('bare-super-home-open')
    mark=key('meta_l');desktop('Terminal',mark,False)
    mark=key('meta_l','spc');desktop('Terminal',mark,True)
    capture('super-space-search-open')
    key('esc')  # Search Escape returns to Home in this frozen model.
    mark=key('meta_l','ret');desktop('Terminal',mark,False)
    # Prove shortcut letters never append to a pending terminal command.
    key('p',frame=False);key('meta_l','e');mark=key('meta_l','ret');desktop('Terminal',mark)
    key('s',frame=False);mark=key('ret')
    wait_text('NEXT PROCESS: pid=',mark)
    # Real conditional Copy; default cancellation leaves existing destination.
    command('write /native/copy-src seed','NEXT DATA: Wrote 4 bytes')
    command('write /native/copy-dst old','NEXT DATA: Wrote 3 bytes')
    select('/native/copy-src');key('c',frame=False);key('right')
    replace_path('/native/copy-src','/native/copy-dst')
    mark=click('dialog_apply');found=wait_regex(rb'PRISM FILE INSPECT: request=\d+ path=/native/copy-dst result=Ok\(Some',mark);wait_text('PRISM FRAME:',found.end())
    capture('copy-overwrite-default-cancel');key('ret')
    assert_bytes('/native/copy-src',b'seed');assert_bytes('/native/copy-dst',b'old')
    click('file_copy');replace_path('/native/copy-src','/native/copy-dst')
    mark=click('dialog_apply');found=wait_regex(rb'PRISM FILE INSPECT: request=\d+ path=/native/copy-dst result=Ok\(Some',mark);wait_text('PRISM FRAME:',found.end())
    mark=click('dialog_apply');commit=wait_regex(rb'PRISM FILE COMMIT: request=\d+ intent=Copy[^\r\n]*result=Ok\(\(\)\)',mark);wait_text('PRISM FRAME:',commit.end())
    assert_bytes('/native/copy-src',b'seed');assert_bytes('/native/copy-dst',b'seed');capture('copy-confirmed-real-directory')
    # Dirty AltF4 Cancel, actual save-before-close, then discard preserving disk.
    command('edit /native/copy-src','PRISM EDITOR OPEN: path=/native/copy-src bytes=4 exists=true')
    key('end',frame=False);key('x',frame=False);key('right')
    mark=key('alt','f4');desktop('Editor',mark);shown=capture('alt-f4-dirty-default-cancel')
    ex,ey,_,_=geometry['editor']
    if base.crop(shown,(ex+13,ey+97,1,1))!=base.colors(helper,'dark','violet')['accent']:raise RuntimeError('dirty close is not visibly default Cancel')
    key('ret');mark=key('ctrl','s',frame=False);wait_text('PRISM EDITOR SAVE: path=/native/copy-src bytes=5',mark);key('left');assert_bytes('/native/copy-src',b'seedx')
    key('end',frame=False);key('x',frame=False);key('right');key('alt','f4');key('left')
    mark=key('ret');wait_text('PRISM EDITOR SAVE: path=/native/copy-src bytes=6',mark);assert_bytes('/native/copy-src',b'seedxx')
    mark=key('right');closed=wait_regex(rb'PRISM DESKTOP: focused=Some\((?!Editor)[A-Za-z]+\) home=false[^\r\n]*',mark);wait_text('PRISM FRAME:',closed.end())
    capture('alt-f4-save-before-close')
    command('edit /native/copy-src','PRISM EDITOR OPEN: path=/native/copy-src bytes=6 exists=true')
    key('end',frame=False);key('x',frame=False);key('right');key('alt','f4');key('tab');mark=key('ret')
    closed=wait_regex(rb'PRISM DESKTOP: focused=Some\((?!Editor)[A-Za-z]+\) home=false[^\r\n]*',mark);wait_text('PRISM FRAME:',closed.end())
    assert_bytes('/native/copy-src',b'seedxx');capture('alt-f4-discard-preserves-stored-bytes')
    command('rm /native/copy-src','NEXT DATA: Removed /native/copy-src')
    command('rm /native/copy-dst','NEXT DATA: Removed /native/copy-dst')
    report['checks'].append('actual Super Home/search/app shortcuts preserve pending terminal command; real conditional Copy cancel/overwrite retains source; dirty AltF4 Cancel/Save/Discard agree with stored bytes')
    report['shortcuts_copy']={'captures':captures,'timeline':timeline,'limits':'Real bounded built-in app shortcuts and Copy; no secure-lock/workspace/capture or client input delivery claim.'}

def main():
    # Independent tool/helper identities; baseline ninth evidence is untouched.
    base.HOST_SOURCE=HOST_SOURCE;base.exercise=exercise
    if '--source-manifest' not in sys.argv:sys.argv+=['--source-manifest',str(ROOT/'work/tenth-wave/source-manifest.json')]
    if '--frozen-source-root' not in sys.argv:sys.argv+=['--frozen-source-root',str(ROOT/'work/tenth-wave/frozen-src')]
    if '--output-dir' not in sys.argv:sys.argv+=['--output-dir',str(ROOT/'build/prism-shortcuts-copy')]
    return base.main()
if __name__=='__main__':sys.exit(main())
