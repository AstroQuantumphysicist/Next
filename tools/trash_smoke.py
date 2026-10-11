#!/usr/bin/env python3
"""Source-bound actual GUI Trash and retained native descriptor, two scratch boots.

No image building. Requires an immutable Source11 image and matching retained
source manifest. Host checks cannot substitute for the real guest proof.
"""
import argparse
import json
import math
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
import zlib
import boot_smoke
import prism_files_appearance_smoke as base
ROOT = base.ROOT
HOST_SOURCE = ROOT / 'tools/trash_smoke_host.rs'
PROOF = '/native/trash-proof'
MODIFIED = b'held-modified'


def trash_records(files):
    """Independently verify private pairs, owner and complete canonical records."""
    pairs = {}
    for path, entry in files.items():
        if not path.startswith('/trash/'):
            continue
        match = re.fullmatch(r'/trash/([dm])\.([0-9a-f]{16})', path)
        if not match or int(match[2], 16) == 0:
            raise RuntimeError('noncanonical private Trash key')
        pairs.setdefault(int(match[2], 16), {})[match[1]] = entry['data']
    if len(pairs) > 2:
        raise RuntimeError('Trash item bound exceeded')
    result = []
    for ident, pair in sorted(pairs.items()):
        if set(pair) != {'d', 'm'}:
            raise RuntimeError('orphaned Trash pair')
        record = pair['m']
        if len(record) != 128:
            raise RuntimeError('noncanonical record size')
        n = record[40]
        original = record[41:41+n]
        if (record[:8] != b'NXTRASH1' or record[8:12] != b'\1\0\x80\0'
                or any(record[12:16]) or int.from_bytes(record[16:24], 'little') != 1
                or int.from_bytes(record[24:32], 'little') != ident
                or int.from_bytes(record[32:36], 'little') != len(pair['d'])
                or len(pair['d']) > 4096
                or int.from_bytes(record[36:40], 'little') != zlib.crc32(pair['d']) & 0xffffffff
                or not 9 <= n <= 31 or not original.startswith(b'/native/')
                or re.fullmatch(rb'[A-Za-z0-9/._-]+', original) is None
                or any(part in (b'', b'.', b'..') for part in original[1:].split(b'/'))
                or any(record[41+n:124])
                or int.from_bytes(record[124:128], 'little') != zlib.crc32(record[:124]) & 0xffffffff):
            raise RuntimeError('Trash owner/identity/path/fingerprint/record CRC mismatch')
        result.append({'id': ident, 'original': original.decode('ascii'), 'data': pair['d']})
    return result


def exercise(args, phase, image, helper, binding, original, serial, data, wait_text,
             wait_regex, wait_exit, qmp, remaining, report):
    original(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report)
    if base.digest(args.source_manifest) != binding['compiled_source_manifest_sha256'] or base.digest(helper) != binding['helper_sha256']:
        raise RuntimeError('retained source binding changed')
    width, height = map(int, wait_regex(rb'PRISM: READY (\d+)x(\d+);').groups())
    geometry = {name: tuple(map(int, values)) for name, *values in
                (line.split() for line in subprocess.check_output([str(helper), 'geometry', str(width), str(height)]).decode().splitlines())}
    output = args.output_dir / phase / 'trash'; output.mkdir(parents=True, exist_ok=True)
    pointers = list(re.finditer(rb'PRISM POINTER: x=(\d+) y=(\d+)', data))
    pointer = list(map(int, pointers[-1].groups())) if pointers else [width//2, height//2]
    captures, timeline = [], []
    trash_page = False
    def key(*codes, frame=True):
        mark = len(data); timeline.append({'keys': codes, 'serial_start': mark})
        qmp('send-key', {'keys':[{'type':'qcode','data':code} for code in codes], 'hold-time':40})
        if frame: wait_text('PRISM FRAME:', mark)
        time.sleep(min(.07, remaining())); return mark
    def click(name):
        x,y,w,h = geometry[name]; target = [x+w//2, y+h//2]
        while pointer != target:
            delta = [max(-90, min(90, target[i]-pointer[i])) for i in range(2)]
            qmp('input-send-event', {'events':[{'type':'rel','data':{'axis':axis,'value':delta[i]}} for i,axis in enumerate(('x','y')) if delta[i]]})
            pointer[:] = [pointer[i]+delta[i] for i in range(2)]; time.sleep(min(.02, remaining()))
        mark = len(data); timeline.append({'click':target,'serial_start':mark})
        qmp('input-send-event', {'events':[{'type':'btn','data':{'down':True,'button':'left'}}]})
        found = wait_regex(rb'PRISM POINTER: x=(\d+) y=(\d+) buttons=1', mark)
        if list(map(int,found.groups())) != target: raise RuntimeError('source-bound pointer mismatch')
        mark_release = len(data)
        qmp('input-send-event', {'events':[{'type':'btn','data':{'down':False,'button':'left'}}]})
        event = wait_regex(rb'PRISM POINTER: [^\r\n]*buttons=0[^\r\n]*', mark_release)
        wait_text('PRISM FRAME:', event.end()); return mark
    def capture(name):
        path = output/(name+'.ppm'); qmp('screendump', {'filename':str(path),'format':'ppm'})
        captures.append({'name':name,'sha256':base.digest(path),'path':str(path.relative_to(ROOT))})
        return base.ppm(path.read_bytes())
    def command(text, expected):
        click('terminal_button'); mark = len(data); serial.sendall((text+'\r').encode())
        event = wait_text(expected, mark); return mark
    def state():
        qmp('stop')
        try: return base.journal_state(image, args.store_start)
        finally: qmp('cont')
    def disk():
        return state()[1]
    def select(path):
        nonlocal trash_page
        files = disk()
        names = [name for name,entry in sorted(files.items(),key=lambda p:p[1]['slot']) if not name.startswith('/trash/')]
        if path not in names: raise RuntimeError('selected path absent from actual journal')
        click('files_button')
        if trash_page:
            click('trash_button_2'); trash_page = False
        click('file_row_'+str(names.index(path)+1))
    def replace_path(old,new):
        click('dialog_input')
        for _ in old: key('backspace',frame=False)
        for char in new: key({'/':'slash','-':'minus','.':'dot'}.get(char,char),frame=False)
        key('right')
    def commit(kind, error=None):
        mark = click('dialog_apply')
        expected = rb'Err\('+error.encode()+rb'\)' if error else rb'Ok\(\(\)\)'
        event = wait_regex(rb'PRISM TRASH COMMIT: request=\d+ intent='+kind.encode()+rb'[^\r\n]*result='+expected, mark)
        wait_text('PRISM FRAME:', event.end())
    def show_pair(expected=MODIFIED):
        nonlocal trash_page
        click('files_button'); key('t',frame=False); key('right'); trash_page = True
        items = trash_records(disk())
        if len(items) != 1 or items[0]['original'] != PROOF or items[0]['data'] != expected:
            raise RuntimeError('authoritative Trash pair differs from retained descriptor bytes')
        click('trash_row_0')
        shown = capture('persisted-row-'+phase)
        x,y,_,_ = geometry['trash_row_0']
        template = base.ppm(subprocess.check_output([str(helper), 'text', 'light', 'blue', 'small-selected', PROOF]))
        actual = base.crop(shown, (x+8, y+4, template[0], template[1]))
        background = base.colors(helper, 'light', 'blue')['selected']
        foreground = [i for i in range(0, len(actual), 3) if template[2][i:i+3] != background]
        if not foreground or any(actual[i:i+3] != template[2][i:i+3] for i in foreground):
            raise RuntimeError('actual GUI Trash row did not render original path')
        return items
    if phase == 'create':
        command('runfiles /system/trash-probe.elf','TRASH-PROBE: READY retained descriptor; waiting for trusted GUI move')
        # The developer fixture creates its file after launch. A real command
        # refreshes authoritative GUI bindings after the READY event.
        command('status','NEXT: kernel-control=independent')
        select(PROOF); before_cancel=state(); click('file_delete'); capture('move-default-cancel'); key('ret')
        if state()!=before_cancel: raise RuntimeError('cancelled Move changed committed generation or bytes')
        files = disk()
        if files.get(PROOF,{}).get('data') != b'original-data' or trash_records(files): raise RuntimeError('cancelled Move changed namespace')
        click('file_delete'); commit('Move')
        wait_text('TRASH-PROBE: PASS held inode write/readback after GUI move')
        files = disk()
        if PROOF in files: raise RuntimeError('Move retained original name')
        show_pair()
        # Existing destination collision must leave the private pair untouched.
        command('write '+PROOF+' collision','NEXT DATA: Wrote 9 bytes')
        show_pair(); click('trash_button_0'); commit('Restore','Exists')
        capture('restore-collision-retained-input'); key('ret')  # defaultCancel after error
        if disk().get(PROOF,{}).get('data') != b'collision': raise RuntimeError('Restore overwrote conflicting destination')
        click('trash_button_0'); replace_path(PROOF,'/native/trash-restored'); commit('Restore')
        files = disk()
        if files.get('/native/trash-restored',{}).get('data') != MODIFIED or trash_records(files): raise RuntimeError('Restore did not atomically move data and remove metadata')
        command('rm '+PROOF,'NEXT DATA: Removed '+PROOF)
        command('mv /native/trash-restored '+PROOF,'NEXT DATA: Renamed /native/trash-restored to '+PROOF)
        select(PROOF); click('file_delete'); commit('Move'); show_pair()
        report['checks'].append('real Move cancellation/confirmation, native retained-inode write/readback, collision-preserving Restore and alternative destination agree with checksummed atomic pairs')
    else:
        show_pair(); capture('reboot-authoritative-listing')
        before_cancel=state(); click('trash_button_1'); capture('purge-default-cancel'); key('ret')
        if state()!=before_cancel: raise RuntimeError('cancelled Purge changed committed generation or bytes')
        if len(trash_records(disk())) != 1: raise RuntimeError('Purge cancellation removed item')
        click('trash_button_1'); commit('Purge')
        if trash_records(disk()): raise RuntimeError('confirmed Purge left private record or data')
        shown = capture('purged-empty-listing'); x,y,_,_ = geometry['files']
        if not base.text_matches(helper,shown,x+12,y+118,'Trash is empty.'):
            raise RuntimeError('actual GUI did not refresh empty Trash')
        report['checks'].append('second guest boot renders persisted real Trash listing; Cancel retains pair and confirmed Purge removes both objects')
    report['trash'] = {'phase':phase,'captures':captures,'timeline':timeline,
                       'limits':'Two-item/8-object/4KiB single development owner; explicit trusted runfiles namespace grant; no production consent or full Trash hierarchy claim.'}

    # The inherited baseline next exercises Terminal snapping; restore its
    # focus contract after our real Files interactions.
    click('terminal_button')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", type=Path, default=ROOT / "build/next-dev.img")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "build/trash-smoke")
    parser.add_argument("--source-manifest", type=Path, default=ROOT / "work/eleventh-wave/source-manifest.json")
    parser.add_argument("--frozen-source-root", type=Path, default=ROOT / "work/eleventh-wave/frozen-src")
    parser.add_argument("--timeout", type=float, default=450)
    parser.add_argument("--cpu", default="qemu64")
    parser.add_argument("--store-start", type=int, default=64 * 1024 * 1024 // 512)
    parser.add_argument("--gpt-data-profile", action="store_true")
    args = parser.parse_args()
    args.image, args.output_dir = args.image.resolve(), args.output_dir.resolve()
    args.source_manifest, args.frozen_source_root = args.source_manifest.resolve(), args.frozen_source_root.resolve()
    if not args.output_dir.is_relative_to(ROOT / "build") or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("output must be beneath build/ and timeout finitepositive")
    if args.store_start < 0:
        parser.error("store start must be nonnegative")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    initial = base.digest(args.image)
    base.HOST_SOURCE = HOST_SOURCE
    helper, binding = base.ensure_host(args.source_manifest, args.frozen_source_root, args.output_dir / "host")
    final = {"passed": False, "image_sha256": initial, "source_binding": binding, "phases": []}
    original, old_argv = boot_smoke.exercise_file_portal, sys.argv[:]
    try:
        with tempfile.TemporaryDirectory(prefix="trash-eleven-", dir=ROOT / "work") as directory:
            scratch = Path(directory) / "scratch.img"
            shutil.copyfile(args.image, scratch)
            protected=base.protected_digest(scratch,args.store_start)
            for phase in ("create", "verify"):
                boot_smoke.exercise_file_portal = lambda *hook, phase=phase: exercise(args, phase, scratch, helper, binding, original, *hook)
                sys.argv = [str(ROOT / "tools/boot_smoke.py"), "--image", str(scratch), "--output-dir", str(args.output_dir / phase), "--timeout", str(args.timeout), "--cpu", args.cpu, "--persistent-data"]
                if args.gpt_data_profile:
                    sys.argv.append("--gpt-data-profile")
                status = boot_smoke.main()
                report = json.loads((args.output_dir / phase / "boot-smoke.json").read_text())
                final["phases"].append({"phase": phase, "passed": status == 0 and report.get("passed") is True,
                                        "report": str((args.output_dir / phase / "boot-smoke.json").relative_to(ROOT))})
                if not final["phases"][-1]["passed"]:
                    raise RuntimeError(f"{phase} actual guest phase failed: {report.get('error')}")
                if base.protected_digest(scratch,args.store_start)!=protected:raise RuntimeError("guest changed bytes outside authorized journal prefix")
                final["phases"][-1]["protected_bytes_unchanged"]=True
                journal, files = base.journal_state(scratch, args.store_start)
                items = trash_records(files)
                if (phase == "create" and (len(items) != 1 or items[0]["data"] != MODIFIED)) or (phase == "verify" and items):
                    raise RuntimeError("clean guest shutdown changed expected Trash pair")
                final["phases"][-1]["journal"] = journal
            final["passed"] = True
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        final["error"] = str(error)
    finally:
        sys.argv, boot_smoke.exercise_file_portal = old_argv, original
        final["input_image_unchanged"] = base.digest(args.image) == initial
        final["source_manifest_unchanged"] = base.digest(args.source_manifest) == binding["compiled_source_manifest_sha256"]
        final["helper_source_unchanged"] = base.digest(HOST_SOURCE)==binding["helper_source_sha256"]
        if not final["input_image_unchanged"] or not final["source_manifest_unchanged"] or not final["helper_source_unchanged"]:
            final["passed"] = False
            final["error"] = "immutable input image or compiled-source manifest changed"
        (args.output_dir / "trash-smoke.json").write_text(json.dumps(final, indent=2) + "\n")
    print(json.dumps(final, indent=2))
    return 0 if final["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
