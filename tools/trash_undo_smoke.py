#!/usr/bin/env python3
"""Source12 real GUI Undo extension to the source-bound two-boot Trash gate.

Never builds an image. Checks committed journal bytes/pairs, cancellation,
Move/Restore inverses, one-shot receipts, Purge invalidation and reboot reset.
No guest pass is implied by host syntax or geometry checks.
"""
from pathlib import Path
import re
import json
import subprocess
import sys
import time
import trash_smoke as trash

ROOT = trash.ROOT
HOST_SOURCE = ROOT / 'tools/trash_undo_smoke_host.rs'
original_exercise = trash.exercise
RESTORED = '/native/undo-restored'


def extension(args, phase, image, helper, serial, data, wait_text, wait_regex,
              qmp, remaining, report, stage):
    width,height = map(int, wait_regex(rb'PRISM: READY (\d+)x(\d+);').groups())
    geometry = {name:tuple(map(int, values)) for name,*values in
                (line.split() for line in subprocess.check_output(
                    [str(helper),'geometry',str(width),str(height)]).decode().splitlines())}
    actions=[];captures=[]
    output=args.output_dir/phase/'undo';output.mkdir(parents=True,exist_ok=True)
    def frame_after(mark):
        return wait_regex(rb'PRISM (?:OUTPUT )?FRAME: generation=(\d+)',mark)
    def key(code):
        mark=len(data)
        qmp('send-key',{'keys':[{'type':'qcode','data':code}],'hold-time':40})
        actions.append({'key':code,'serial_start':mark,'monotonic_ns':time.monotonic_ns()})
        return mark
    def click(name):
        b=geometry[name];target=[b[0]+b[2]//2,b[1]+b[3]//2]
        def move(delta):
            mark=len(data)
            events=[{'type':'rel','data':{'axis':axis,'value':delta[i]}}
                    for i,axis in enumerate(('x','y')) if delta[i]]
            events.append({'type':'btn','data':{'button':'middle','down':True}})
            qmp('input-send-event',{'events':events})
            actual=wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=4',mark)
            frame_after(actual.end())
            release=len(data)
            qmp('input-send-event',{'events':[{'type':'btn','data':{'button':'middle','down':False}}]})
            event=wait_regex(rb'PRISM POINTER: [^\r\n]*buttons=0[^\r\n]*',release)
            frame_after(event.end())
            return list(map(int,actual.groups()))
        pos=move([0,0])
        for _ in range(64):
            if pos==target:break
            pos=move([max(-90,min(90,target[i]-pos[i])) for i in range(2)])
        else:raise RuntimeError('actual PS/2 feedback did not converge')
        mark=len(data)
        qmp('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':True}}]})
        actual=wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=1',mark)
        if list(map(int,actual.groups()))!=target:raise RuntimeError('actual pointer missed control')
        release=len(data)
        qmp('input-send-event',{'events':[{'type':'btn','data':{'button':'left','down':False}}]})
        event=wait_regex(rb'PRISM POINTER: [^\r\n]*buttons=0[^\r\n]*',release)
        frame_after(event.end());return mark
    def disk():
        qmp('stop')
        try:return trash.base.journal_state(image,args.store_start)
        finally:qmp('cont')
    def capture(name):
        path=output/(name+'.ppm');qmp('screendump',{'filename':str(path),'format':'ppm'})
        captures.append({'path':str(path.relative_to(ROOT)),'sha256':trash.base.digest(path)})
    def open_trash():
        click('files_button');key('t');mark=key('right');frame_after(mark)
    def assert_pair(files,expected_id=None):
        items=trash.trash_records(files)
        if len(items)!=1 or items[0]['original']!=trash.PROOF or items[0]['data']!=trash.MODIFIED:
            raise RuntimeError('actual private pair differs from expected inverse bytes')
        if expected_id is not None and items[0]['id']!=expected_id:
            raise RuntimeError('Undo Restore changed exact retained Trash identity')
        return items[0]
    def undo(available):
        before=disk();mark=key('u')
        if available:
            event=wait_regex(rb'PRISM TRASH UNDO: request=(\d+) generation=(\d+) result=Ok\(\(\)\)',mark)
            # Character U need not emit FRAME; a harmless Right forces a
            # completed frame strictly after the authoritative Undo response.
            redraw=key('right');frame_after(max(redraw,event.end()))
        else:
            redraw=key('right');frame_after(redraw)
            if b'PRISM TRASH UNDO:' in data[mark:]:raise RuntimeError('disabled Undo dispatched a backend action')
            if disk()!=before:raise RuntimeError('unavailable Undo mutated journal bytes/generation')
    def commit(intent):
        mark=click('dialog_apply')
        event=wait_regex(rb'PRISM TRASH COMMIT: request=\d+ intent='+intent.encode()+rb'[^\r\n]*result=Ok\(\(\)\)',mark)
        redraw=key('right');frame_after(max(redraw,event.end()))
    def select_file(path):
        _,files=disk();names=[name for name,entry in sorted(files.items(),key=lambda p:p[1]['slot']) if not name.startswith('/trash/')]
        if path not in names:raise RuntimeError('file selection absent from journal')
        click('files_button');click('trash_button_2');mark=key('right');frame_after(mark)
        click('file_row_'+str(names.index(path)+1))
    def move_file():
        select_file(trash.PROOF);click('file_delete');commit('Move');open_trash()
        return assert_pair(disk()[1])
    def restore(destination=None):
        click('trash_row_0');click('trash_button_0')
        if destination:
            click('dialog_input')
            for _ in trash.PROOF:key('backspace')
            for char in destination:key({'/':'slash','-':'minus','.':'dot'}.get(char,char))
            mark=key('right');frame_after(mark)
        commit('Restore')
    if stage=='after' and phase=='create':
        # The baseline hook restores Terminal focus for subsequent snap checks.
        # Undo is a Files/Trash action, so explicitly focus its actual page.
        open_trash()
        initial=assert_pair(disk()[1]);undo(True)
        _,files=disk()
        if files.get(trash.PROOF,{}).get('data')!=trash.MODIFIED or trash.trash_records(files):
            raise RuntimeError('Undo Move did not restore exact original bytes')
        capture('undo-move-restored')
        moved=move_file();before=disk()
        click('trash_row_0');click('trash_button_0');mark=key('ret');frame_after(mark)
        if disk()!=before:raise RuntimeError('Cancel Restore mutated pair/generation')
        undo(True)  # Cancel must preserve the eligible Move receipt.
        if disk()[1].get(trash.PROOF,{}).get('data')!=trash.MODIFIED:
            raise RuntimeError('Cancel invalidated eligible Undo Move')
        moved=move_file();restore(RESTORED)
        if disk()[1].get(RESTORED,{}).get('data')!=trash.MODIFIED or trash.trash_records(disk()[1]):
            raise RuntimeError('Restore did not move complete pair into destination')
        open_trash();undo(True)
        _,files=disk();assert_pair(files,moved['id'])
        if RESTORED in files:raise RuntimeError('Undo Restore retained restored destination')
        capture('undo-restore-exact-pair');undo(False)
        # Leave a fresh Move receipt eligible at shutdown: reboot must clear
        # the session receipt while retaining the actual checksummed pair.
        restore();move_file();capture('eligible-receipt-before-reboot')
        report['checks'].append('real U Undo Move and Undo Restore preserve exact bytes/Trash identity; Cancel preserves receipt and consumed Undo cannot run twice')
    elif stage=='before' and phase=='verify':
        open_trash();assert_pair(disk()[1]);undo(False);capture('reboot-no-session-undo')
        report['checks'].append('new guest boot retains checksummed Trash bytes but cannot replay the prior session Undo receipt')
    elif stage=='after' and phase=='verify':
        if trash.trash_records(disk()[1]):raise RuntimeError('baseline confirmed Purge retained pair')
        open_trash();undo(False);capture('purge-no-undo')
        report['checks'].append('confirmed Purge removes both private objects and cannot be undone')
    report.setdefault('trash_undo',{'actions':[],'captures':[],'scope':'Single session bounded inverse receipts; actual journal inspection; no general directory/history/desktop Undo claim.'})
    # Restore the inherited baseline's Terminal snapping/focus contract.
    click('terminal_button')
    report['trash_undo']['actions'].extend(actions);report['trash_undo']['captures'].extend(captures)


def exercise(args,phase,image,helper,binding,original,serial,data,wait_text,
             wait_regex,wait_exit,qmp,remaining,report):
    def before(*hook):
        original(*hook)
        extension(args,phase,image,helper,serial,data,wait_text,wait_regex,qmp,remaining,report,'before')
    original_exercise(args,phase,image,helper,binding,before if phase=='verify' else original,
                      serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report)
    extension(args,phase,image,helper,serial,data,wait_text,wait_regex,qmp,remaining,report,'after')


def main():
    # Pair compiled source geometry with the explicit prepared binary identity;
    # a newer source helper must never silently validate another default image.
    expected=None
    forwarded=[sys.argv[0]]
    image=ROOT/'build/next-dev.img'
    index=1
    while index<len(sys.argv):
        arg=sys.argv[index]
        if arg=='--expected-image-sha256':
            if index+1>=len(sys.argv):raise SystemExit('missing expected image digest')
            expected=sys.argv[index+1];index+=2;continue
        if arg.startswith('--expected-image-sha256='):
            expected=arg.split('=',1)[1];index+=1;continue
        if arg=='--image' and index+1<len(sys.argv):image=Path(sys.argv[index+1]).resolve()
        elif arg.startswith('--image='):image=Path(arg.split('=',1)[1]).resolve()
        forwarded.append(arg);index+=1
    if expected is None:
        expected=json.loads((ROOT/'work/twelfth-wave/prepare.json').read_text())['sha256']['next-dev.img']
    if trash.base.digest(image)!=expected:
        raise SystemExit('Loaded image differs from expected prepared Source12 identity; no VM launched')
    sys.argv=forwarded
    trash.HOST_SOURCE=HOST_SOURCE;trash.exercise=exercise
    for flag,value in [('--source-manifest',ROOT/'work/twelfth-wave/source-manifest.json'),
                       ('--frozen-source-root',ROOT/'work/twelfth-wave/frozen-src'),
                       ('--output-dir',ROOT/'build/trash-undo')]:
        if not any(arg==flag or arg.startswith(flag+'=') for arg in sys.argv):sys.argv.extend([flag,str(value)])
    return trash.main()

if __name__=='__main__':sys.exit(main())
