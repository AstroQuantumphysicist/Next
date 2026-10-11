#!/usr/bin/env python3
"""Actual trusted resize→native configure/ack/full-frame gate, source-bound."""
import hashlib,json,re,subprocess,sys
from pathlib import Path
import native_window_smoke as windows
import surface_capability_smoke as surface
PACKAGE='org.next.development.native.client-reconfigure-probe'
FIXTURE='crates/next-user/src/bin/client-reconfigure-probe.rs'
CONFIG=(rb'PRISM CLIENT CONFIGURATION PRESENTED: frame=(\d+) slot=(\d+) generation=(\d+) configuration=(\d+) pending=(\d+)\r?\n')
RESIZE=(rb'PRISM CLIENT RESIZE: request=(\d+) slot=(\d+) generation=(\d+) pid=(\d+) old_configuration=(\d+) configuration=(\d+) width=(\d+) height=(\d+) stride=(\d+) applied=(true|false)\r?\n')

def checker(image,bounds,pw,ph,new):
    w,h,pixels=image;x,y,dw,dh=bounds
    if min(x,y)<0 or min(dw,dh)<1 or x+dw>w or y+dh>h: raise RuntimeError('actual configured content lies outside captured display')
    colors=[bytes.fromhex(c) for c in (('30d030','a050a0') if new else ('3060b0','f0c020'))]
    for row in range(dh):
        expected=b''.join(colors[((column*pw//dw)//8+(row*ph//dh)//8)%2] for column in range(dw))
        offset=((y+row)*w+x)*3
        if pixels[offset:offset+dw*3]!=expected: raise RuntimeError('actual configured frame differs from literal nearest-neighbor app checkerboard')

def exercise(serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report,original):
    output=surface.ROOT/'build/native-resize'
    for i,a in enumerate(sys.argv[1:]):
        if a=='--output-dir': output=Path(sys.argv[i+2]).resolve()
        elif a.startswith('--output-dir='): output=Path(a.split('=',1)[1]).resolve()
    output.mkdir(parents=True,exist_ok=True)
    manifest_path=Path(surface.BINDING['manifest_path']);sources=json.loads(manifest_path.read_text());frozen=manifest_path.parent/'frozen-src'
    for name,markers in {FIXTURE:('SurfaceSession','raw_publish','dynamic full checkerboard'), 'crates/helix-kernel/src/main.rs':('PRISM CLIENT RESIZE','PRISM CLIENT CONFIGURATION PRESENTED')}.items():
        p=frozen/name
        if name not in sources or hashlib.sha256(p.read_bytes()).hexdigest()!=sources[name] or any(m not in p.read_text() for m in markers): raise RuntimeError('retained resize producer/fixture binding mismatch')
    host,binding=windows.ensure_host(manifest_path,output/'host',surface.ROOT/'tools/native_resize_host.rs')
    captures=[];transactions=[];dimensions=None
    start=len(data);serial.sendall(b'run /system/client-reconfigure-probe.elf\r')
    m=wait_regex(rb'HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/client-reconfigure-probe\.elf profile=Native',start);pid=int(m[1])
    ident=wait_regex(rb'HELIX: APPLICATION IDENTITY pid='+str(pid).encode()+rb' package=([^ ]+) signer=',start)
    if ident[1].decode()!=PACKAGE:raise RuntimeError('resize fixture actual signed identity mismatch')
    m=wait_regex(rb'PRISM SURFACE: slot=(\d+) generation=(\d+) frame=1 pid='+str(pid).encode()+rb' package='+PACKAGE.encode()+rb'\r?\n',start)
    slot,gen=map(int,m.groups());configuration=1;pw=64;ph=48;accepted=1
    def latest():
        records=[m for m in re.finditer(windows.PRESENT,data[start:]) if int(m[2])==slot and int(m[3])==gen]
        if not records:raise RuntimeError('no post-copy exact native-window placement')
        return dict(zip(('frame','slot','generation','mapped','minimized','x','y','width','height','z','focused'),map(int,records[-1].groups())))
    def screenshot(name):
        nonlocal dimensions
        p=output/(name+'.ppm');qmp('screendump',{'filename':str(p),'format':'ppm'});raw=p.read_bytes();image=surface.ppm(raw);dimensions=image[:2]
        captures.append({'path':str(p.relative_to(surface.ROOT)),'sha256':hashlib.sha256(raw).hexdigest()});return image
    def geometry():
        if dimensions is None:screenshot('initial-display')
        p=latest();text=subprocess.check_output([str(host),*map(str,dimensions),str(slot),str(gen),str(pid),PACKAGE,*map(str,(p['x'],p['y'],p['width'],p['height'],configuration,pw,ph))],text=True)
        return {row.split()[0]:tuple(map(int,row.split()[1:])) for row in text.splitlines()}
    def packet(dx=0,dy=0,buttons=0):
        mark=len(data);events=[{'type':'rel','data':{'axis':a,'value':v}} for a,v in (('x',dx),('y',dy)) if v]
        events += [{'type':'btn','data':{'button':b,'down':bool(buttons&bit)}} for b,bit in (('left',1),('right',2),('middle',4))]
        if not dx and not dy:events.append({'type':'rel','data':{'axis':'x','value':1}})
        qmp('input-send-event',{'events':events});m=wait_regex(rb'PRISM POINTER PRESENTED: generation=\d+ x=(\d+) y=(\d+) buttons='+str(buttons).encode()+rb'\r?\n',mark)
        return list(map(int,m.groups()))
    def move(target):
        position=packet()
        for _ in range(40):
            if position==target:return
            before=position;position=packet(*[max(-90,min(90,target[i]-position[i])) for i in range(2)])
            if position==before:raise RuntimeError('actual motion stalled')
        raise RuntimeError('actual motion exceeded bounded budget')
    def click(b):
        move([b[0]+b[2]//2,b[1]+b[3]//2]);mark=len(data);packet(buttons=1);packet(buttons=0);return mark
    # Move the window away from the viewport edge before growing it. This
    # changes trusted outer placement, not native configuration or focus policy.
    title=geometry()['title'];move([title[0]+title[2]//2,title[1]+title[3]//2]);packet(buttons=1)
    for _ in range(3):packet(-70,0,1)
    packet(buttons=0)
    checker(screenshot('initial-owned-checkerboard'),geometry()['content'],pw,ph,False)
    for label,dx,dy in (('grow',65,45),('shrink',-35,-25)):
        old=configuration;mark=len(data);b=geometry()['resize'];move([b[0]+b[2]//2,b[1]+b[3]//2]);packet(buttons=1);packet(dx,dy,1);packet(buttons=0)
        r=wait_regex(RESIZE,mark);v=list(map(int,r.groups()[:-1]))+[int(r[10]==b'true')]
        if (v[1],v[2],v[3],v[4],v[9])!=(slot,gen,pid,old,1) or v[5]<=old or v[8]!=v[6]*4 or v[6]*v[7]*4>65536:raise RuntimeError('trusted resize transaction identity/budget mismatch')
        configuration=v[5];pw=v[6];ph=v[7]
        queued=wait_regex(rb'NEXT APP pid='+str(pid).encode()+rb': CLIENT RECONFIGURE FRAME: configure='+str(configuration).encode()+rb' frame=(\d+) width='+str(pw).encode()+rb' height='+str(ph).encode()+rb' stride='+str(v[8]).encode()+rb' bytes=(\d+)\r?\n',mark)
        if int(queued[1])<=accepted or int(queued[2])!=pw*ph*4:raise RuntimeError('app frame counter or exact packed size regressed')
        accepted=int(queued[1]);wait_text(f'PRISM SURFACE: slot={slot} generation={gen} frame={accepted} pid={pid} package={PACKAGE}',queued.start())
        pending=wait_regex(CONFIG.replace(rb'slot=(\d+)',rb'slot=('+str(slot).encode()+rb')').replace(rb'generation=(\d+)',rb'generation=('+str(gen).encode()+rb')').replace(rb'configuration=(\d+)',rb'configuration=('+str(configuration).encode()+rb')').replace(rb'pending=(\d+)',rb'pending=(0)'),mark)
        wait_text(f'NEXT APP pid={pid}: CLIENT RECONFIGURE PROBE: dynamic full checkerboard accepted; raw stale config/frame/buffer checks passed; fresh focus required',mark)
        if re.search(rb'PRISM CLIENT FOCUS: .*pid='+str(pid).encode()+rb' accepted=true',data[r.end():pending.end()]):raise RuntimeError('configuration/ack/publication acquired input focus automatically')
        checker(screenshot(label+'-owned-checkerboard'),geometry()['content'],pw,ph,True)
        transactions.append({'request':v[0],'configuration':configuration,'pixel_width':pw,'pixel_height':ph,'frame':accepted,'presented_frame':int(pending[1])})
    controls=geometry()
    focus=click(controls['content']);wait_text(f'PRISM CLIENT FOCUS: slot={slot} generation={gen} pid={pid} accepted=true',focus)
    exit_start=len(data);qmp('send-key',{'keys':[{'type':'qcode','data':'q'}],'hold-time':40});wait_exit(pid,52,exit_start)
    wait_text(f'PRISM SURFACE CLEARED: slot={slot} generation={gen} pid={pid}',exit_start)
    for name in ('home','files','settings'):
        click(controls[name]);screenshot('actual-desktop-'+name)
        if name=='home':click(controls[name])
    click(controls['terminal'])
    report['native_resize']={'pid':pid,'slot':slot,'generation':gen,'package':PACKAGE,'fixture_sha256':sources[FIXTURE],'producer_sha256':sources['crates/helix-kernel/src/main.rs'],'validator_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'host_binding':binding,'transactions':transactions,'captures':captures,'exit_code':52,
      'limits':'Trusted kernel software compositor and explicit user resize gestures; no isolated compositor, GPU fence, private capture policy or physical key-release claim.'}
    report['checks'].extend(['actual trusted resize commits new configure, native Reset/ack and monotonically newer full-damage checkerboard frame','raw stale configuration/frame, old-size buffer and closed handle are denied in original signed fixture; reconfigure does not acquire focus','completed guest screenshots show dynamic owned geometry plus actual Home/Files/Settings; real input exit clears exact generation'])
    return original(serial,data,wait_text,wait_regex,wait_exit,qmp,remaining,report)

def main():
    if '--self-test' in sys.argv:
        resize=re.search(RESIZE,b'PRISM CLIENT RESIZE: request=7 slot=0 generation=3 pid=43 old_configuration=1 configuration=2 width=80 height=60 stride=320 applied=true\r\n')
        assert resize and tuple(map(int,resize.groups()[:-1]))==(7,0,3,43,1,2,80,60,320) and resize[10]==b'true'
        configuration=re.search(CONFIG,b'PRISM CLIENT CONFIGURATION PRESENTED: frame=100 slot=0 generation=3 configuration=2 pending=0\r\n')
        assert configuration and tuple(map(int,configuration.groups()))==(100,0,3,2,0)
        # Non-integral display scaling must use the same nearest-neighbor
        # coordinates as the actual client renderer, including edge pixels.
        for pw,ph,dw,dh,new in ((16,16,16,16,False),(80,60,357,268,True),(70,52,322,239,True)):
            rgb=[bytes.fromhex(x) for x in (('30d030','a050a0') if new else ('3060b0','f0c020'))]
            pixels=b''.join(rgb[((x*pw//dw)//8+(y*ph//dh)//8)%2] for y in range(dh) for x in range(dw))
            checker((dw,dh,pixels),(0,0,dw,dh),pw,ph,new)
            damaged=pixels[:-3]+b'\x00\x00\x00'
            try:checker((dw,dh,damaged),(0,0,dw,dh),pw,ph,new)
            except RuntimeError:pass
            else:raise AssertionError('damaged checkerboard edge accepted')
        try:checker((16,16,b'\x00'*(16*16*3)),(-1,0,16,16),16,16,False)
        except RuntimeError:pass
        else:raise AssertionError('out-of-display crop accepted')
        print('Resize transaction parsers and literal scaled checkerboard checks PASS; no guest boot performed');return 0
    surface.exercise_surface=exercise
    if '--require-guarded-emergency-stack' not in sys.argv:sys.argv.append('--require-guarded-emergency-stack')
    return surface.main()
if __name__=='__main__':sys.exit(main())
