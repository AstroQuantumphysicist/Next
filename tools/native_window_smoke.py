#!/usr/bin/env python3
"""Source-bound real ordinary native-window gestures and completed guest pixels."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import surface_capability_smoke as surface

PACKAGE = 'org.next.development.native.client-input-probe'
PRESENT = (rb'PRISM NATIVE WINDOW PRESENTED: frame=(\d+) slot=(\d+) generation=(\d+) '
           rb'mapped=(\d+) minimized=(\d+) x=(-?\d+) y=(-?\d+) width=(\d+) height=(\d+) z=(\d+) focused=(\d+)\r?\n')

def ensure_host(manifest_path, output, probe_path=None):
    manifest_path=Path(manifest_path); sources=json.loads(manifest_path.read_text())
    retained=manifest_path.parent/'frozen-src'
    hashes={p:h for p,h in sources.items() if p.startswith('crates/prism/')}
    if not hashes or 'crates/prism/src/native_window_shell.rs' not in hashes:
        raise RuntimeError('retained source does not contain the ordinary native-window UI')
    for p,h in hashes.items():
        if hashlib.sha256((retained/p).read_bytes()).hexdigest()!=h:
            raise RuntimeError('retained Prism source/asset hash mismatch: '+p)
    for p in (retained/'crates/prism').rglob('*'):
        if p.is_file() and p.relative_to(retained).as_posix() not in hashes:
            raise RuntimeError('unmanifested retained Prism file')
    rustc=shutil.which('rustc')
    if not rustc: raise RuntimeError('source tools/env.sh: rustc unavailable')
    output.mkdir(parents=True,exist_ok=True)
    library=output/'libprism.rlib'; binary=output/'native-window-host'
    probe=Path(probe_path) if probe_path else surface.ROOT/'tools/native_window_host.rs'
    subprocess.run([rustc,'--edition=2021','--crate-type=rlib','--crate-name=prism','-Cpanic=abort',str(retained/'crates/prism/src/lib.rs'),'-o',str(library)],check=True)
    subprocess.run([rustc,'--edition=2021','-Cpanic=abort',str(probe),'--extern','prism='+str(library),'-o',str(binary)],check=True)
    binding={'manifest_sha256':hashlib.sha256(manifest_path.read_bytes()).hexdigest(), 'prism_sources':hashes,
             'probe_sha256':hashlib.sha256(probe.read_bytes()).hexdigest(), 'library_sha256':hashlib.sha256(library.read_bytes()).hexdigest(),
             'helper_sha256':hashlib.sha256(binary.read_bytes()).hexdigest()}
    (output/'host-binding.json').write_text(json.dumps(binding,indent=2)+'\n')
    return binary,binding

def exercise(serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report,original):
    output=surface.ROOT/'build/native-window'
    for i,arg in enumerate(sys.argv[1:]):
        if arg=='--output-dir': output=Path(sys.argv[i+2]).resolve()
        elif arg.startswith('--output-dir='): output=Path(arg.split('=',1)[1]).resolve()
    output.mkdir(parents=True,exist_ok=True)
    manifest_path=Path(surface.BINDING['manifest_path'])
    retained=manifest_path.parent/'frozen-src'
    sources=json.loads(manifest_path.read_text())
    for name,markers in {
        'crates/helix-kernel/src/main.rs': ('PRISM NATIVE WINDOW PRESENTED','PRISM POINTER PRESENTED'),
        'crates/next-user/src/bin/client-input-probe.rs': ('client_input_fixture',),
        'crates/next-user/src/client_input_fixture.rs': ('creation/publication did not focus',),
    }.items():
        p=retained/name
        if name not in sources or hashlib.sha256(p.read_bytes()).hexdigest()!=sources[name] or any(m not in p.read_text() for m in markers):
            raise RuntimeError('native-window producer/fixture differs from retained manifest')
    host,binding=ensure_host(manifest_path,output/'host')
    clients=[]; captures=[]; gestures=[]
    dimensions=None
    def state(match):
        v=list(map(int,match.groups()))
        return dict(zip(('frame','slot','generation','mapped','minimized','x','y','width','height','z','focused'),v))
    def latest(client):
        matches=[m for m in re.finditer(PRESENT,data[client['start']:]) if int(m[2])==client['slot'] and int(m[3])==client['generation']]
        if not matches: raise RuntimeError('no completed exact-generation native placement')
        return state(matches[-1])
    def placement(client,start,mapped,minimized):
        pattern=PRESENT.replace(rb'slot=(\d+)',rb'slot=('+str(client['slot']).encode()+rb')').replace(rb'generation=(\d+)',rb'generation=('+str(client['generation']).encode()+rb')')
        pattern=pattern.replace(rb'mapped=(\d+)',rb'mapped=('+str(mapped).encode()+rb')').replace(rb'minimized=(\d+)',rb'minimized=('+str(minimized).encode()+rb')')
        return state(wait_regex(pattern,start))
    def screenshot(name):
        nonlocal dimensions
        path=output/(name+'.ppm'); qmp('screendump',{'filename':str(path),'format':'ppm'})
        raw=path.read_bytes(); image=surface.ppm(raw); dimensions=image[:2]
        captures.append({'path':str(path.relative_to(surface.ROOT)),'sha256':hashlib.sha256(raw).hexdigest()})
        return image
    def geometry(client):
        if dimensions is None: screenshot('geometry-initial')
        p=latest(client)
        text=subprocess.check_output([str(host),*map(str,dimensions),str(client['slot']),str(client['generation']),str(client['pid']),PACKAGE,*map(str,(p['x'],p['y'],p['width'],p['height']))],text=True)
        return {line.split()[0]:tuple(map(int,line.split()[1:])) for line in text.splitlines()}
    def literal(client,color,name,present=True):
        width,height,pixels=screenshot(name); b=geometry(client)['content']
        x,y,w,h=b
        if min(x,y)<0 or x+w>width or y+h>height or min(w,h)<16: raise RuntimeError('owned pixels outside actual geometry')
        found=all(pixels[((y+r)*width+x+3)*3:((y+r)*width+x+w-3)*3]==bytes.fromhex(color)*(w-6) for r in range(3,11))
        if found!=present: raise RuntimeError('completed ordinary-window owned pixels disagree with mapped state')
    def packet(dx=0,dy=0,buttons=0):
        start=len(data)
        events=[{'type':'rel','data':{'axis':axis,'value':v}} for axis,v in (('x',dx),('y',dy)) if v]
        events.extend({'type':'btn','data':{'button':b,'down':bool(buttons&bit)}} for b,bit in (('left',1),('right',2),('middle',4)))
        # Ensure an unchanged mask still produces actual feedback.
        if not dx and not dy: events.append({'type':'rel','data':{'axis':'x','value':1}})
        qmp('input-send-event',{'events':events})
        m=wait_regex(rb'PRISM POINTER PRESENTED: generation=\d+ x=(\d+) y=(\d+) buttons='+str(buttons).encode()+rb'\r?\n',start)
        return list(map(int,m.groups()))
    def move(target,buttons=0):
        pos=packet(buttons=buttons)
        for _ in range(40):
            if pos==target: return pos
            old=pos; pos=packet(*[max(-90,min(90,target[i]-pos[i])) for i in range(2)],buttons)
            if pos==old: raise RuntimeError('PS/2 feedback motion made no progress')
        raise RuntimeError('bounded PS/2 motion failed to reach actual control')
    def click(bounds):
        target=[bounds[0]+bounds[2]//2,bounds[1]+bounds[3]//2]; move(target)
        mark=len(data); packet(buttons=1); packet(buttons=0)
        gestures.append({'kind':'click','target':target,'serial_start':mark}); return mark
    def key(code): qmp('send-key',{'keys':[{'type':'qcode','data':code}],'hold-time':40})
    def focus(client):
        already_focused=bool(latest(client)['focused'])
        mark=click(geometry(client)['content'])
        if not already_focused:
            wait_text(f"PRISM CLIENT FOCUS: slot={client['slot']} generation={client['generation']} pid={client['pid']} accepted=true",mark)
        # Repeated same-owner gestures intentionally produce no FocusGain.
        # The following real key/owned frame verifies current routing instead.
    def character(client,code,color):
        start=len(data); key(code)
        e=wait_regex(rb'NEXT APP pid='+str(client['pid']).encode()+rb': CLIENT INPUT EVENT: seq=(\d+) configure=\d+ kind=3 x=0 y=0 buttons=0 scalar='+str(ord(code)).encode()+rb' timestamp=\d+',start)
        wait_text(f"PRISM SURFACE: slot={client['slot']} generation={client['generation']} frame={int(e[1])+1} pid={client['pid']} package={PACKAGE}",e.start())
        if any(re.search(rb'NEXT APP pid='+str(c['pid']).encode()+rb': CLIENT INPUT EVENT: .*kind=3 .*scalar='+str(ord(code)).encode()+rb' ',data[start:]) for c in clients if c is not client): raise RuntimeError('background client received key')
        literal(client,color,f"pid-{client['pid']}-{code}-{len(captures)}")
    def launch():
        start=len(data); serial.sendall(b'run /system/client-input-probe.elf\r')
        s=wait_regex(rb'HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/client-input-probe\.elf profile=Native',start); pid=int(s[1])
        ident=wait_regex(rb'HELIX: APPLICATION IDENTITY pid='+str(pid).encode()+rb' package=([^ ]+) signer=',start)
        if ident[1].decode()!=PACKAGE: raise RuntimeError('client principal mismatch')
        frame=wait_regex(rb'PRISM SURFACE: slot=(\d+) generation=(\d+) frame=1 pid='+str(pid).encode()+rb' package='+PACKAGE.encode()+rb'\r?\n',start)
        c={'pid':pid,'slot':int(frame[1]),'generation':int(frame[2]),'start':start}; clients.append(c)
        if re.search(rb'PRISM CLIENT FOCUS: .*pid='+str(pid).encode()+rb' accepted=true',data[start:frame.end()]): raise RuntimeError('publication stole focus')
        placement(c,start,1,0); return c
    first=launch(); second=launch()
    focus(first); character(first,'a','30d030')
    before=latest(first); title=geometry(first)['title']; target=[title[0]+title[2]//2,title[1]+title[3]//2]; move(target)
    mark=len(data); packet(buttons=1)
    for _ in range(4): packet(-70,20,1)
    packet(buttons=0)
    moved=placement(first,mark,1,0)
    moved=latest(first)
    if (moved['x'],moved['y'])==(before['x'],before['y']) or (moved['width'],moved['height'])!=(before['width'],before['height']): raise RuntimeError('real titlebar drag failed or changed client extent')
    focus(first); character(first,'a','30d030')
    if latest(first)['z'] <= latest(second)['z']: raise RuntimeError('drag/focus did not raise the native window in completed metadata')
    mark=click(geometry(first)['minimize']); placement(first,mark,0,1)
    if re.search(rb'PRISM SURFACE CLEARED: slot='+str(first['slot']).encode()+rb' generation='+str(first['generation']).encode(),data[mark:]): raise RuntimeError('minimize destroyed surface generation')
    # Use old content geometry for the unmapped-pixel check; helper still models
    # placement, independently of the real hidden state.
    literal(first,'30d030','minimized-owned-pixels-absent',False)
    literal(first,'707070','minimized-latest-focus-loss-pixels-absent',False)
    alive=len(data); serial.sendall(b'ps\r')
    wait_regex(rb'NEXT PROCESS: pid='+str(first['pid']).encode()+rb' state=(?:runnable|waiting|sleeping) [^\r\n]+\r?\n',alive)
    wait_text('PRISM FRAME:',alive)
    mark=click(geometry(first)['restore']); placement(first,mark,1,0)
    character(first,'a','30d030')
    focus(second); character(second,'b','f0c020')
    if latest(second)['z'] <= latest(first)['z']: raise RuntimeError('foreground native window z-order did not change after real gesture')
    overlay=len(data); key('o')
    wait_regex(rb'PRISM PORTAL: request=\d+ pid='+str(second['pid']).encode()+rb' access=1\r?\n',overlay)
    key('f10'); wait_text('PRISM FRAME:',overlay)
    guarded=len(data); click(geometry(first)['title']); key('a'); key('f10'); wait_text('PRISM FRAME:',guarded)
    if re.search(rb'NEXT APP pid=\d+: CLIENT INPUT EVENT: .*kind=3 .*scalar=97 ',data[guarded:]): raise RuntimeError('secure file portal leaked client input')
    screenshot('secure-portal-priority'); key('esc')
    authenticated_clients=[dict(c) for c in clients]
    for c in (second,first):
        focus(c); mark=len(data); key('q'); wait_exit(c['pid'],50,mark)
        wait_text(f"PRISM SURFACE CLEARED: slot={c['slot']} generation={c['generation']} pid={c['pid']}",mark)
        clients.remove(c)
    report['native_windows']={'package':PACKAGE,'clients':authenticated_clients,'validator_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'fixture_bindings':{name:sources[name] for name in ('crates/next-user/src/bin/client-input-probe.rs','crates/next-user/src/client_input_fixture.rs')},'host_binding':binding,'captures':captures,'gestures':gestures,'moved':moved,
      'limits':'Trusted kernel software Prism; move/minimize/restore and focus only, no production resize/isolated compositor/physical-key-release claim.'}
    report['checks'].extend(['source-bound real titlebar drag moves exact-generation owned pixels without resizing','minimize hides owned pixels while process/capability survive; trusted restore shows latest owned publication','real native window gestures route only foreground keys and secure file portal retains priority'])
    return original(serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report)

def main():
    if '--self-test' in sys.argv:
        m=re.search(PRESENT,b'PRISM NATIVE WINDOW PRESENTED: frame=17 slot=0 generation=2 mapped=0 minimized=1 x=10 y=12 width=292 height=240 z=7 focused=0\r\n')
        assert m and tuple(map(int,m.groups()))==(17,0,2,0,1,10,12,292,240,7,0)
        print('Native window metadata parser PASS; no guest boot performed'); return 0
    surface.exercise_surface=exercise
    if '--require-guarded-emergency-stack' not in sys.argv:
        sys.argv.append('--require-guarded-emergency-stack')
    return surface.main()
if __name__=='__main__': sys.exit(main())
