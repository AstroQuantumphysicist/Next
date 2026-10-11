#!/usr/bin/env python3
"""Source-bound actual Prism Files/Appearance PS2 gate on two owned scratch boots.

Never builds an image. Requires a retained ninth Prism source snapshot matching
its source manifest. The immutable input image is cloned beneath work/. Actual
journal bytes are verified after QEMU stops; no host test substitutes guest input.
"""
import argparse
import hashlib
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
from cow_verify import verify as verify_cow

ROOT = boot_smoke.ROOT
HOST_SOURCE = ROOT / "tools/prism_files_appearance_host.rs"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def ensure_host(manifest, frozen, output):
    expected = {p: h for p, h in json.loads(manifest.read_text()).items()
                if p.startswith("crates/prism/")}
    if not expected:
        raise RuntimeError("source manifest contains no Prism sources")
    for path, value in expected.items():
        if not (frozen / path).is_file() or digest(frozen / path) != value:
            raise RuntimeError(f"retained ninth source differs from manifest: {path}")
    actual = {p.relative_to(frozen).as_posix() for p in (frozen / "crates/prism").rglob("*") if p.is_file()}
    if actual != set(expected):
        raise RuntimeError("retained Prism crate has missing or unmanifested files")
    rustc = shutil.which("rustc")
    if not rustc:
        raise RuntimeError("source tools/env.sh first: rustc is unavailable")
    output.mkdir(parents=True, exist_ok=True)
    library, helper = output / "libprism.rlib", output / "prism-files-appearance-host"
    subprocess.run([rustc, "--edition=2021", "--crate-name=prism", "--crate-type=rlib", "-Cpanic=abort",
                    str(frozen / "crates/prism/src/lib.rs"), "-o", str(library)], check=True)
    subprocess.run([rustc, "--edition=2021", "-Cpanic=abort", str(HOST_SOURCE), "--extern", f"prism={library}", "-o", str(helper)], check=True)
    binding = {"compiled_source_manifest_sha256": digest(manifest), "frozen_source_root": str(frozen),
               "prism_sources": expected, "helper_source_sha256": digest(HOST_SOURCE),
               "library_sha256": digest(library), "helper_sha256": digest(helper)}
    (output / "host-binding.json").write_text(json.dumps(binding, indent=2) + "\n")
    return helper, binding


def ppm(raw):
    header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", raw)
    if not header:
        raise RuntimeError("invalid PPM header")
    width, height = int(header[1]), int(header[2])
    data = raw[header.end():]
    if len(data) != width * height * 3:
        raise RuntimeError("invalid PPM length")
    return width, height, data


def crop(image, bounds):
    width, height, pixels = image
    x, y, w, h = bounds
    if min(x, y, w, h) < 0 or x + w > width or y + h > height:
        raise RuntimeError("source-bound geometry exceeds guest framebuffer")
    return b"".join(pixels[((y + row) * width + x) * 3:((y + row) * width + x + w) * 3] for row in range(h))


def colors(helper, theme, accent):
    return {name: bytes.fromhex(rgb) for name, rgb in
            (line.split() for line in subprocess.check_output([str(helper), "palette", theme, accent]).decode().splitlines())}


def text_matches(helper, image, x, y, text, theme="light", accent="blue", style="body"):
    template = ppm(subprocess.check_output([str(helper), "text", theme, accent, style, text]))
    actual = crop(image, (x, y, template[0], template[1]))
    palette = colors(helper, theme, accent)
    background = palette["raised" if style == "field" else "surface"]
    foreground = [i for i in range(0, len(actual), 3) if template[2][i:i + 3] != background]
    return bool(foreground) and all(actual[i:i + 3] == template[2][i:i + 3] for i in foreground)


def record_bytes(theme=1, accent=1):
    raw = bytearray(32)
    raw[:8] = b"NXAPREF1"
    raw[8:10], raw[10:12] = (1).to_bytes(2, "little"), (32).to_bytes(2, "little")
    raw[12], raw[13], raw[14:16] = theme, accent, (100).to_bytes(2, "little")
    raw[28:32] = (zlib.crc32(raw[:28]) & 0xffffffff).to_bytes(4, "little")
    return bytes(raw)


def journal_state(image, start):
    result = verify_cow(image, start, 132, [(0, start)])
    if result["mount_read_only"]:
        raise RuntimeError("scratch journal unexpectedly mounted read-only")
    records = {}
    with image.open("rb") as stream:
        for row, item in enumerate(sorted(result["files"], key=lambda entry: entry["slot"]), 1):
            stream.seek((start + 2 + result["selected_bank"] * 65 + item["slot"] * 8) * 512)
            payload = stream.read(item["size"])
            if zlib.crc32(payload) & 0xffffffff != item["crc32"]:
                raise RuntimeError("verified metadata and actual payload disagree")
            records[item["path"]] = {**item, "data": payload, "gui_row": row}
    return result, records


def protected_digest(image,start):
    digest=hashlib.sha256()
    with image.open("rb") as stream:
        remaining=start*512
        while remaining:
            chunk=stream.read(min(remaining,1024*1024))
            if not chunk:raise RuntimeError("truncated protected prefix")
            digest.update(chunk);remaining-=len(chunk)
        stream.seek((start+132)*512)
        for chunk in iter(lambda:stream.read(1024*1024),b""):digest.update(chunk)
    return digest.hexdigest()


def exercise(args, phase, image, helper, binding, original, serial, data, wait_text,
             wait_regex, wait_exit, qmp, remaining, report):
    original(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report)
    if digest(args.source_manifest) != binding["compiled_source_manifest_sha256"] or digest(helper) != binding["helper_sha256"]:
        raise RuntimeError("source/helper binding changed during boot")
    dimensions = wait_regex(rb"PRISM: READY (\d+)x(\d+);")
    width, height = int(dimensions[1]), int(dimensions[2])
    if width < 1024 or height < 768:
        raise RuntimeError("this full Files interaction gate requires at least1024x768; small viewport has a separate model gate")
    geometry = {name: tuple(map(int, values)) for name, *values in
                (line.split() for line in subprocess.check_output([str(helper), "geometry", str(width), str(height)]).decode().splitlines())}
    output = args.output_dir / phase / "files-appearance"
    output.mkdir(parents=True, exist_ok=True)
    captures, timeline = [], []
    pointers=list(re.finditer(rb"PRISM POINTER: x=(\d+) y=(\d+)", data))
    pointer = list(map(int, pointers[-1].groups())) if pointers else [width//2,height//2]
    active_theme, active_accent = ("light", "blue") if phase == "create" else ("dark", "violet")

    def screenshot(name):
        path = output / (name + ".ppm")
        qmp("screendump", {"filename": str(path), "format": "ppm"})
        captures.append({"name": name, "sha256": digest(path), "path": str(path.relative_to(ROOT))})
        return ppm(path.read_bytes())

    def key(*codes, frame=True):
        mark = len(data)
        timeline.append({"input": "keys", "keys": codes, "host_monotonic_ns": time.monotonic_ns(), "serial_start": mark})
        qmp("send-key", {"keys": [{"type": "qcode", "data": code} for code in codes], "hold-time": 40})
        if frame:
            wait_text("PRISM FRAME:", mark)
        time.sleep(min(0.07, remaining()))
        return mark

    def type_text(text):
        for char in text:
            code = {"/": "slash", "-": "minus", " ": "spc", ".": "dot"}.get(char, char)
            if not re.fullmatch(r"[a-z0-9/ .-]", char):
                raise RuntimeError("test text contains an unsupported original PS2 fixture key")
            key(code, frame=False)
        key("right")  # ignored by Files form, provides an ordered completed frame

    def click(name):
        bounds = geometry[name] if isinstance(name, str) else name
        target = [bounds[0] + bounds[2] // 2, bounds[1] + bounds[3] // 2]
        while pointer != target:
            delta = [max(-90, min(90, target[i] - pointer[i])) for i in range(2)]
            qmp("input-send-event", {"events": [{"type": "rel", "data": {"axis": axis, "value": delta[i]}}
                                                  for i, axis in enumerate(("x", "y")) if delta[i]]})
            pointer[:] = [pointer[i] + delta[i] for i in range(2)]
            time.sleep(min(0.02, remaining()))
        mark = len(data)
        timeline.append({"input": "click", "position": target, "host_monotonic_ns": time.monotonic_ns(), "serial_start": mark})
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"down": True, "button": "left"}}]})
        event = wait_regex(rb"PRISM POINTER: x=(\d+) y=(\d+) buttons=1", mark)
        if list(map(int, event.groups())) != target:
            raise RuntimeError("pointer differs from source-bound action geometry")
        released = len(data)
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"down": False, "button": "left"}}]})
        release = wait_regex(rb"PRISM POINTER: x=\d+ y=\d+ buttons=0[^\r\n]*", released)
        wait_text("PRISM FRAME:", release.end())
        return mark

    def command(text, expected):
        click("terminal_button")
        mark = len(data)
        serial.sendall((text + "\r").encode("ascii"))
        response = wait_regex(re.escape(expected.encode("ascii")), mark)
        wait_text("PRISM FRAME:", response.end())
        timeline.append({"input": "console", "command": text, "response": expected, "serial_start": mark})
        return mark

    def disk():
        qmp("stop")
        try:
            return journal_state(image, args.store_start)
        finally:
            qmp("cont")

    def select_file(path):
        _, files = disk()
        if path not in files:
            raise RuntimeError(f"real backend does not contain selected GUI file: {path}")
        click("files_button")
        return click(f"file_row_{files[path]['gui_row']}")

    def replace_path(old, new):
        click("dialog_input")
        for _ in old:
            key("backspace", frame=False)
        type_text(new)

    def confirmed_commit(mark, operation):
        event = wait_regex(rb"PRISM FILE COMMIT: request=(\d+) intent=" + operation.encode()
                           + rb"[^\r\n]* result=([^\r\n]+)", mark)
        if event[2] != b"Ok(())":
            raise RuntimeError(f"actual {operation} backend rejected operation: {event[2].decode()}")
        wait_text("PRISM FRAME:", event.end())
        timeline.append({"backend": operation, "request": int(event[1]), "serial_start": mark, "result": "Ok"})

    if phase == "verify":
        wait_text("PRISM APPEARANCE LOAD: origin=Persisted accepted=true")
        _, files = disk()
        if files.get("/prefs/appearance", {}).get("data") != record_bytes():
            raise RuntimeError("second guest boot did not retain the exact saved32-byte preference record")
        click("settings_button")
        shown = screenshot("reboot-dark-violet")
        sx, sy, _, _ = geometry["settings"]
        if not text_matches(helper, shown, sx + 12, sy + 244, "Saved appearance is active.", active_theme, active_accent, "muted"):
            raise RuntimeError("rebooted authoritative preferences did not render as active")
        report["checks"].append("second clean guest boot loads exact persistedDark/Violet100%CRC record and renders active preferences")
    else:
        click("settings_button")
        sx, sy, _, _ = geometry["settings"]
        screenshot("appearance-light")
        for theme, index, accent, accent_index in [("dark", 1, "violet", 1), ("contrast", 2, "orange", 3), ("light", 0, "blue", 0), ("dark", 1, "violet", 1)]:
            click(f"appearance_0_{index}")
            click(f"appearance_1_{accent_index}")
            mark = click("appearance_2_0")
            saved = wait_regex(rb"PRISM APPEARANCE SAVE: request=(\d+) theme=([^ ]+) accent=([^ ]+) result=(.*?) accepted=(true|false)", mark)
            if saved[4] != b"Ok(())" or saved[5] != b"true":
                raise RuntimeError(f"actual Appearance persistence failed: {saved[0].decode()}")
            if saved[2].decode()!= {"light":"Light","dark":"Dark","contrast":"HighContrast"}[theme] or saved[3].decode()!=accent.title():
                raise RuntimeError("backend committed a different requested appearance")
            wait_text("PRISM FRAME:", saved.end())
            active_theme, active_accent = theme, accent
            shown = screenshot("appearance-" + theme + "-" + accent)
            if not text_matches(helper, shown, sx + 12, sy + 244, "Appearance saved and active.", theme, accent, "muted"):
                raise RuntimeError("successful preference commit did not render actual saved state")
            palette = colors(helper, theme, accent)
            save_button = geometry["appearance_2_0"]
            if crop(shown, (save_button[0] + save_button[2] // 2, save_button[1] + 4, 1, 1)) != palette["background"]:
                raise RuntimeError("real neutralbutton uses wrong semantic palette")
        _, files = disk()
        if files.get("/prefs/appearance", {}).get("data") != record_bytes():
            raise RuntimeError("appearance UI success differs from real committed record")
        report["checks"].append("actual Theme/Accent mouse edits save Light/Dark/HighContrast before activation and persist exactCRC-protected preferences")

        click("files_button")
        click("file_create")
        type_text("ui-a")
        screenshot("create-path")
        mark = click("dialog_apply")
        confirmed_commit(mark, "Create")
        command("cat /native/ui-a", "NEXT DATA: ")
        _, files = disk()
        if files.get("/native/ui-a", {}).get("data") != b"":
            raise RuntimeError("GUI create did not commit an empty real file")
        mark = select_file("/native/ui-a")
        wait_text("PRISM EDITOR OPEN: path=/native/ui-a bytes=0 exists=true", mark)
        for char in "alpha":
            key(char, frame=False)
        mark = key("ctrl", "s", frame=False)
        wait_text("PRISM EDITOR SAVE: path=/native/ui-a bytes=5", mark)
        key("left")
        command("cat /native/ui-a", "NEXT DATA: alpha")
        command("write /native/ui-b beta", "NEXT DATA: Wrote 4 bytes")
        click("files_button")
        click("file_rename")
        replace_path("/native/ui-a", "/native/ui-b")
        mark = click("dialog_apply")
        wait_regex(rb"PRISM FILE INSPECT: request=\d+ path=/native/ui-b result=Ok\(Some", mark)
        shown = screenshot("overwrite-cancel-default")
        dx, dy, _, _ = geometry["dialog"]
        if not text_matches(helper, shown, dx + 20, dy + 18, "Replace destination file?", active_theme, active_accent, "title"):
            raise RuntimeError("existing destination did not require real overwrite confirmation")
        key("ret")  # Cancel is the default, even after destination inspection.
        command("cat /native/ui-a", "NEXT DATA: alpha")
        command("cat /native/ui-b", "NEXT DATA: beta")
        click("files_button")
        click("file_rename")
        replace_path("/native/ui-a", "/native/ui-b")
        click("dialog_apply")
        mark = click("dialog_apply")
        confirmed_commit(mark, "Rename")
        command("cat /native/ui-a", "Data store error: NotFound")
        command("cat /native/ui-b", "NEXT DATA: alpha")
        _, files = disk()
        if "/native/ui-a" in files or files.get("/native/ui-b", {}).get("data") != b"alpha":
            raise RuntimeError("confirmed replacement differs from actual coherent file bytes")
        click("files_button")
        click("file_delete")
        shown = screenshot("permanent-delete-cancel-default")
        if not text_matches(helper, shown, dx + 20, dy + 18, "Permanently delete file?", active_theme, active_accent, "title"):
            raise RuntimeError("Delete lacks permanent-impact confirmation")
        key("ret")
        command("cat /native/ui-b", "NEXT DATA: alpha")
        click("files_button")
        click("file_delete")
        mark = click("dialog_apply")
        confirmed_commit(mark, "Delete")
        command("cat /native/ui-b", "Data store error: NotFound")
        report["checks"].append("real GUI Create, EditorCtrlS, cancel/confirmed replacement Rename and cancel/permanent Delete agree with console bytes and committedjournal")

        select_file("/prefs/appearance")
        click("file_rename")
        replace_path("/prefs/appearance", "/prefs/appearance2")
        mark = click("dialog_apply")
        denied = wait_regex(rb"PRISM FILE INSPECT: request=\d+ path=/prefs/appearance2 result=Err\(ReadOnly\)", mark)
        wait_text("PRISM FRAME:", denied.end())
        shown = screenshot("rename-policy-error-preserves-input")
        for x, y, text, style in [(dx + 20, dy + 52, "/prefs/appearance", "muted"), (dx + 30, dy + 98, "/prefs/appearance2", "field"), (dx + 20, dy + 152, "This store is read-only.", "error")]:
            if not text_matches(helper, shown, x, y, text, active_theme, active_accent, style):
                raise RuntimeError("real ReadOnly error changed selected source/input or hid readable error")
        key("esc")
        key("shift", "tab")
        focused = screenshot("shift-tab-visible-create-focus")
        b = geometry["file_create"]
        if crop(focused, (b[0] + b[2] // 2, b[1] + b[3] - 2, 1, 1)) != colors(helper, active_theme, active_accent)["accent"]:
            raise RuntimeError("ShiftTab did not restore predictable visibleCreate focus")
        # Establish a visible immediate App::ALL successor rather than guessing
        # which windows the earlier portal/editor gate left open.
        mark=click("monitor_button")
        wait_regex(rb"PRISM POINTER: [^\r\n]*focused=Some\(Monitor\)",mark)
        mark=click("files_button")
        wait_regex(rb"PRISM POINTER: [^\r\n]*focused=Some\(Files\)",mark)
        mark=key("alt", "tab")
        wait_text("PRISM DESKTOP: focused=Some(Monitor) home=false",mark)
        switched=screenshot("alt-tab-window-switch")
        sx,sy,_,_=geometry["monitor"]
        if not text_matches(helper,switched,sx+42,sy+10,"Next Monitor",active_theme,active_accent):
            raise RuntimeError("AltTab did not present focused Monitor title")
        mark=key("alt", "shift", "tab")
        wait_text("PRISM DESKTOP: focused=Some(Files) home=false",mark)
        restored=screenshot("alt-shift-tab-window-restore")
        fx,fy,_,_=geometry["files"]
        if not text_matches(helper,restored,fx+42,fy+10,"Prism Files",active_theme,active_accent):
            raise RuntimeError("AltShiftTab did not present restored Files title")
        _, files = disk()
        if "/prefs/appearance2" in files or files.get("/prefs/appearance", {}).get("data") != record_bytes():
            raise RuntimeError("failed GUI policy operation modified protected preference bytes")
        report["checks"].append("actual native-only ReadOnly GUI error retains source/input and32-byte record; ShiftTab visible focus and AltTab/AltShiftTab window navigation")
    click("terminal_button")
    report["files_appearance"] = {"phase": phase, "viewport": [width, height], "binding": binding,
                                  "geometry": geometry, "captures": captures, "timeline": timeline,
                                  "limits": "Actual bounded Store operations and persisted theme/accent; text scaling/AT transport are not implemented. Held-inode replacement and injected-generation conflicts have separate coherent backend host tests."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", type=Path, default=ROOT / "build/next-dev.img")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "build/prism-files-appearance")
    parser.add_argument("--source-manifest", type=Path, default=ROOT / "work/ninth-wave/source-manifest.json")
    parser.add_argument("--frozen-source-root", type=Path, default=ROOT / "work/ninth-wave/frozen-src")
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
    initial = digest(args.image)
    helper, binding = ensure_host(args.source_manifest, args.frozen_source_root, args.output_dir / "host")
    final = {"passed": False, "image_sha256": initial, "source_binding": binding, "phases": []}
    original, old_argv = boot_smoke.exercise_file_portal, sys.argv[:]
    try:
        with tempfile.TemporaryDirectory(prefix="prism-nine-", dir=ROOT / "work") as directory:
            scratch = Path(directory) / "scratch.img"
            shutil.copyfile(args.image, scratch)
            protected=protected_digest(scratch,args.store_start)
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
                if protected_digest(scratch,args.store_start)!=protected:raise RuntimeError("guest changed bytes outside authorized journal prefix")
                final["phases"][-1]["protected_bytes_unchanged"]=True
                journal, files = journal_state(scratch, args.store_start)
                if files.get("/prefs/appearance", {}).get("data") != record_bytes():
                    raise RuntimeError("clean guest shutdown changed exact persisted preferences")
                final["phases"][-1]["journal"] = journal
            final["passed"] = True
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        final["error"] = str(error)
    finally:
        sys.argv, boot_smoke.exercise_file_portal = old_argv, original
        final["input_image_unchanged"] = digest(args.image) == initial
        final["source_manifest_unchanged"] = digest(args.source_manifest) == binding["compiled_source_manifest_sha256"]
        final["helper_source_unchanged"] = digest(HOST_SOURCE)==binding["helper_source_sha256"]
        if not final["input_image_unchanged"] or not final["source_manifest_unchanged"] or not final["helper_source_unchanged"]:
            final["passed"] = False
            final["error"] = "immutable input image or compiled-source manifest changed"
        (args.output_dir / "files-appearance.json").write_text(json.dumps(final, indent=2) + "\n")
    print(json.dumps(final, indent=2))
    return 0 if final["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
