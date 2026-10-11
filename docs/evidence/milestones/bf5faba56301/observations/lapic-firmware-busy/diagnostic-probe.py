import hashlib,json,os,pathlib,shutil,socket,subprocess,tempfile,time
root=pathlib.Path('/workspace/Next');out=root/'work/lapic-busy-diagnostic';image=root/'build/next-dev.img'
assert hashlib.sha256(image.read_bytes()).hexdigest()=='112baaf5758b81d215f8232d5fb3ae29ea93bf90fecbe96c39e72ecbff59d61d'
sysroot=pathlib.Path('/workspace/toolchains/sysroot');fw=sysroot/'usr/share/OVMF';qemu=sysroot/'usr/bin/qemu-system-x86_64';env=os.environ.copy();env['LD_LIBRARY_PATH']=str(sysroot/'usr/lib/x86_64-linux-gnu')
with tempfile.TemporaryDirectory(prefix='next-lapic-diagnostic-',dir='/tmp') as folder:
    temp=pathlib.Path(folder);shutil.copyfile(fw/'OVMF_VARS_4M.fd',temp/'vars.fd')
    args=[str(qemu),'-machine','q35,accel=tcg','-cpu','qemu64','-m','256M','-smp','1','-vga','std','-drive',f'if=pflash,format=raw,readonly=on,file={fw/"OVMF_CODE_4M.fd"}','-drive',f'if=pflash,format=raw,file={temp/"vars.fd"}','-drive',f'if=none,id=nextdisk,format=raw,snapshot=on,file={image}','-device','virtio-blk-pci,drive=nextdisk,disable-modern=on','-netdev','user,id=nextnet','-device','virtio-net-pci,netdev=nextnet,disable-modern=on','-audiodev',f'wav,id=nextaudio,path={out/"audio.wav"},out.frequency=48000,out.channels=2,out.format=s16','-device','AC97,audiodev=nextaudio','-display','none','-serial',f'file:{out/"serial.log"}','-qmp',f'unix:{temp/"qmp.sock"},server=on,wait=off','-no-reboot']
    for data in ['usr/share/qemu','usr/share/seabios','usr/lib/ipxe/qemu']:
        if (sysroot/data).exists():args+=['-L',str(sysroot/data)]
    with (out/'qemu.log').open('wb') as log:
        p=subprocess.Popen(args,stdout=log,stderr=subprocess.STDOUT,env=env)
        try:
            deadline=time.monotonic()+50
            while not (temp/'qmp.sock').exists():
                assert p.poll() is None
                if time.monotonic()>deadline:raise TimeoutError()
                time.sleep(.05)
            channel=socket.socket(socket.AF_UNIX);channel.connect(str(temp/'qmp.sock'));channel.settimeout(5);stream=channel.makefile('rb');stream.readline()
            def command(name,arguments=None):
                request={'execute':name,'id':name}
                if arguments:request['arguments']=arguments
                channel.sendall(json.dumps(request).encode()+b'\n')
                while True:
                    result=json.loads(stream.readline())
                    if result.get('id')==name:return result
            command('qmp_capabilities')
            while b'LAPIC TIMER UNAVAILABLE Busy' not in (out/'serial.log').read_bytes():
                assert p.poll() is None
                if time.monotonic()>deadline:raise TimeoutError()
                time.sleep(.1)
            command('stop')
            result=command('human-monitor-command',{'command-line':'info lapic'})
            (out/'lapic.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
            command('quit')
        finally:
            if p.poll() is None:p.terminate()
            p.wait(timeout=5)
assert hashlib.sha256(image.read_bytes()).hexdigest()=='112baaf5758b81d215f8232d5fb3ae29ea93bf90fecbe96c39e72ecbff59d61d'
