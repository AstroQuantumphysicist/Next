#!/usr/bin/env python3
"""Actual QMP acceptance for nested Files folders, empty-folder move/remove and reboot persistence.

The immutable image is copied to a disposable scratch disk. Actual PS/2/QMP input
drives Prism; a separately compiled source-bound read-only Store oracle checks the
guest journal after every transaction and after a second boot. Never builds an image.
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

import boot_smoke
import prism_files_appearance_smoke as base

ROOT = boot_smoke.ROOT
HOST_SOURCE = ROOT / "tools/files_directory_host.rs"
KERNEL_SOURCES = {
    "crates/helix-kernel/src/main.rs": ("PRISM DIRECTORY ACTION", "take_files_directory_action"),
    "crates/helix-kernel/src/file.rs": ("directory_action", "gui_directories::perform"),
    "crates/helix-kernel/src/gui_directories.rs": ("Intent::MoveFolder", "rename_between_snapshots"),
    "crates/helix-kernel/src/filesystem.rs": ("DEVELOPMENT_VOLUME_OWNER", "with_directory_store"),
}
def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def ensure_host(manifest, frozen, output):
    records = json.loads(manifest.read_text())
    wanted = {p: h for p, h in records.items()
              if p.startswith(("crates/prism/", "crates/next-fs/")) or p in KERNEL_SOURCES}
    for crate in ("crates/prism/", "crates/next-fs/"):
        expected = {p for p in wanted if p.startswith(crate)}
        actual = {p.relative_to(frozen).as_posix() for p in (frozen / crate).rglob("*") if p.is_file()}
        if not expected or actual != expected:
            raise RuntimeError(f"frozen manifest does not exactly cover {crate}")
    for relative, value in wanted.items():
        if digest(frozen / relative) != value:
            raise RuntimeError(f"retained guest source differs from manifest: {relative}")
    for relative, markers in KERNEL_SOURCES.items():
        source = (frozen / relative).read_text()
        if any(marker not in source for marker in markers):
            raise RuntimeError(f"retained guest directory producer has unexpected contents: {relative}")
    output.mkdir(parents=True, exist_ok=True)
    rustc = shutil.which("rustc")
    if not rustc:
        raise RuntimeError("source tools/env.sh first; rustc is unavailable")
    libs = {}
    for name, crate in (("next_fs", "next-fs"), ("prism", "prism")):
        library = output / f"lib{name}.rlib"
        subprocess.run([rustc, "--edition=2021", "--crate-name", name, "--crate-type=rlib",
                        "-Cpanic=abort", str(frozen / f"crates/{crate}/src/lib.rs"), "-o", str(library)], check=True)
        libs[name] = library
    generated = output / "files_directory_host.rs"
    source = HOST_SOURCE.read_text()
    for crate in ("next-fs", "prism"):
        marker = f'#[path = "../crates/{crate}/src/lib.rs"]'
        # The helper links these crates as rlibs, so it has no relative source-module markers.
        if marker in source:
            raise RuntimeError("unexpected helper source module marker")
    generated.write_text(source)
    helper = output / "files-directory-host"
    subprocess.run([rustc, "--edition=2021", "-Dwarnings", "-Cpanic=abort", str(generated),
                    "--extern", f"next_fs={libs['next_fs']}", "--extern", f"prism={libs['prism']}",
                    "-o", str(helper)], check=True)
    binding = {"manifest_sha256": digest(manifest), "sources": wanted,
               "directory_producer_markers": KERNEL_SOURCES,
               "helper_source_sha256": digest(HOST_SOURCE),
               "validator_sha256": digest(ROOT / "tools/files_directory_smoke.py"),
               "helper_sha256": digest(helper),
               "libraries": {name: digest(path) for name, path in libs.items()}}
    (output / "host-binding.json").write_text(json.dumps(binding, indent=2) + "\n")
    return helper, binding


def oracle(helper, image, start, path):
    raw = subprocess.check_output([str(helper), "list", str(image), str(start), path], text=True)
    lines = raw.splitlines()
    folder = lines[0].split(maxsplit=3)
    if len(folder) != 4 or folder[0] != "folder":
        raise RuntimeError("malformed explicit-directory oracle listing")
    entries = {}
    for line in lines[1:]:
        row = line.split(maxsplit=4)
        if len(row) != 5 or row[0] != "entry":
            raise RuntimeError("malformed explicit-directory oracle row")
        entries[row[4]] = {"kind": row[1], "id": int(row[2]), "size": int(row[3])}
    return {"path": folder[3], "id": int(folder[1]), "revision": int(folder[2]), "entries": entries}


def self_test():
    action = re.search(
        rb"PRISM DIRECTORY ACTION: request=(\d+) kind=(.*?) result=(Ok\((\d+)\)|Err\(([^)]+)\))\r?\n",
        b"PRISM DIRECTORY ACTION: request=9 kind=RemoveFolder { source: FolderSnapshot } result=Err(NotEmpty)\r\n",
    )
    assert action and int(action[1]) == 9 and b"RemoveFolder" in action[2] and action[4] is None and action[5] == b"NotEmpty"
    print("Directory action/rejection parser PASS; no guest boot performed")


def exercise(args, phase, image, helper, binding, serial, data, wait_text, wait_regex,
             wait_exit, qmp, remaining, report):
    output = args.output_dir / phase
    output.mkdir(parents=True, exist_ok=True)
    dimensions = wait_regex(rb"PRISM: READY (\d+)x(\d+);")
    width, height = map(int, dimensions.groups())
    if width < 1024 or height < 768:
        raise RuntimeError("directory UI acceptance requires a 1024x768 or larger guest viewport")
    geometry = {row[0]: tuple(map(int, row[1:])) for row in
                (line.split() for line in subprocess.check_output(
                    [str(helper), "geometry", str(width), str(height)], text=True).splitlines())}
    timeline, captures, actions = [], [], []
    pointer_match = list(re.finditer(rb"PRISM POINTER: x=(\d+) y=(\d+)", data))
    pointer = list(map(int, pointer_match[-1].groups())) if pointer_match else [width // 2, height // 2]

    def frame(mark):
        return wait_regex(rb"PRISM (?:OUTPUT )?FRAME: generation=\d+", mark)

    def key(code, need_frame=True):
        mark = len(data)
        timeline.append({"input": "key", "key": code, "host_monotonic_ns": time.monotonic_ns(), "serial_start": mark})
        qmp("send-key", {"keys": [{"type": "qcode", "data": code}], "hold-time": 40})
        if need_frame:
            frame(mark)
        time.sleep(min(0.055, remaining()))
        return mark

    def type_text(text):
        codes = {"/": "slash", "-": "minus", "_": "shift-minus", ".": "dot"}
        for char in text:
            if char not in "abcdefghijklmnopqrstuvwxyz0123456789/._-":
                raise RuntimeError(f"unsupported PS/2 fixture character: {char!r}")
            key(codes.get(char, char), False)

    def packet(dx=0, dy=0, buttons=0):
        mark = len(data)
        events = [{"type": "rel", "data": {"axis": axis, "value": value}}
                  for axis, value in (("x", dx), ("y", dy)) if value]
        events.extend({"type": "btn", "data": {"button": name, "down": bool(buttons & bit)}}
                      for name, bit in (("left", 1), ("right", 2), ("middle", 4)))
        if not dx and not dy:
            events.append({"type": "rel", "data": {"axis": "x", "value": 1}})
        qmp("input-send-event", {"events": events})
        # The kernel logs raw pointer coordinates only when a button changes.
        # It emits a presentation record for every accepted mouse packet, so
        # wait on that signal for pure movement and button transitions alike.
        match = wait_regex(
            rb"PRISM POINTER PRESENTED: generation=\d+ x=(\d+) y=(\d+) buttons="
            + str(buttons).encode(), mark)
        pointer[:] = map(int, match.groups())

    def click(bounds):
        target = [bounds[0] + bounds[2] // 2, bounds[1] + bounds[3] // 2]
        for _ in range(64):
            if pointer == target:
                break
            delta = [max(-90, min(90, target[i] - pointer[i])) for i in range(2)]
            old = pointer.copy()
            packet(*delta)
            if old == pointer:
                raise RuntimeError("guest mouse pointer stopped before Files taskbar target")
        else:
            raise RuntimeError("guest mouse pointer did not reach Files taskbar target")
        mark = len(data)
        packet(buttons=1)
        packet(buttons=0)
        frame(mark)
        timeline.append({"input": "click", "position": target, "host_monotonic_ns": time.monotonic_ns(), "serial_start": mark})
        return mark

    def screenshot(name):
        path = output / f"{name}.ppm"
        qmp("screendump", {"filename": str(path), "format": "ppm"})
        captures.append({"name": name, "path": str(path.relative_to(ROOT)), "sha256": digest(path)})

    def focus_terminal():
        mark = len(data)
        serial.sendall(b"status\r")
        wait_text("NEXT: kernel-control=independent", mark)
        # The status response proves that the terminal received the command.
        # Focus diagnostics are transition-only, so they may not be repeated
        # when Terminal already owns focus.

    def await_action(mark, intent, ok=True):
        match = wait_regex(rb"PRISM DIRECTORY ACTION: request=(\d+) kind=(.*?) result=(Ok\((\d+)\)|Err\(([^)]+)\))\r?\n", mark)
        if intent.encode() not in match[2]:
            raise RuntimeError(f"actual directory action was not {intent}: {match[2].decode(errors='replace')}")
        if (match[3].startswith(b"Ok(")) != ok:
            raise RuntimeError(f"directory action outcome differed: {match[3]!r}")
        # Backspace navigation is a valid action key but is not among the
        # kernel's diagnostic frame-marker keys. The authoritative action
        # result is emitted only after the backend completion updates Prism.
        record = {"intent": match[2].decode(errors="replace"), "outcome": match[3].decode(), "serial_start": mark}
        actions.append(record)
        return record

    def snapshot(path, expected):
        qmp("stop")
        try:
            got = oracle(helper, image, args.store_start, path)
        finally:
            qmp("cont")
        wanted_paths = set(expected)
        if set(got["entries"]) != wanted_paths:
            raise RuntimeError(f"read-only actual Store listing at {path} differed: {got['entries']!r}")
        for item, wanted in expected.items():
            entry = got["entries"][item]
            wanted_kind, wanted_size = wanted if isinstance(wanted, tuple) else (wanted, 0)
            if entry["kind"] != wanted_kind or entry["size"] != wanted_size or (wanted_kind == "directory" and entry["id"] == 0):
                raise RuntimeError(f"read-only actual Store entry {item} was invalid: {entry!r}")
        return got

    def go(path):
        mark = key("g", False)
        type_text(path)
        mark = key("ret")
        result = await_action(mark, "Navigate", True)
        return result

    def create(name, expected_parent, expected_entries):
        mark = key("n", False)
        type_text(name)
        key("tab")
        mark = key("ret")
        result = await_action(mark, "CreateFolder", True)
        snapshot(expected_parent, expected_entries)
        return result

    def move(destination):
        key("m", False)
        for _ in range(len("/native/")):
            key("backspace", False)
        type_text(destination)
        key("tab")
        mark = key("ret")
        return await_action(mark, "MoveFolder", True)

    def remove(ok=True):
        key("down")
        key("delete")
        if not ok:
            # Cancel is the safe default; confirm only after making impact explicit.
            key("tab")
        key("tab")
        mark = key("ret")
        return await_action(mark, "RemoveFolder", ok)

    # A second independent guest boot proves the explicit namespace survives remount.
    if phase == "verify":
        current = oracle(helper, image, args.store_start, "/native")
        for path in ("/native", "/native/alpha", "/native/beta"):
            listing = oracle(helper, image, args.store_start, path)
            if path == "/native":
                names = {key: value for key, value in listing["entries"].items()}
                if set(names) != {"/native/linux", "/native/probe", "/native/alpha", "/native/beta"}:
                    raise RuntimeError(f"reboot root listing lost/added folders: {names}")
            else:
                if listing["entries"]:
                    raise RuntimeError(f"reboot {path} should be empty after tested removals")
        opened = click(geometry["files_button"])
        wait_regex(rb"PRISM DIRECTORY ACTION: request=\d+ kind=Navigate .*result=Ok\(\d+\)", opened)
        screenshot("files-after-remount")
        report["files_directories"] = {"phase": phase, "reopened_root": current,
                                       "captures": captures, "timeline": timeline,
                                       "checks": ["retained empty sibling folders and their identities survive a full QEMU reboot; removed nested/moved folders remain absent"]}
        report["checks"].append("actual QMP Files directory view remounted persisted nested directory records")
        focus_terminal()
        return

    opened = click(geometry["files_button"])
    await_action(opened, "Navigate", True)
    screenshot("files-root-initial")
    root_files = {"/native/linux": ("file", 23), "/native/probe": ("file", 23)}
    root = snapshot("/native", root_files)
    # Create sibling folders and a nested child through the actual default-cancel dialogs.
    create("alpha", "/native", root_files | {"/native/alpha": "directory"})
    create("beta", "/native", root_files | {"/native/alpha": "directory", "/native/beta": "directory"})
    go("/native/alpha")
    create("deep", "/native/alpha", {"/native/alpha/deep": "directory"})
    nested = snapshot("/native/alpha", {"/native/alpha/deep": "directory"})
    screenshot("files-nested-child-created")
    # Rename in the same parent, then move that exact folder into a sibling parent.
    move("/native/alpha/renamed")
    renamed = snapshot("/native/alpha", {"/native/alpha/renamed": "directory"})
    go("/native/alpha/renamed")
    screenshot("files-renamed-folder-open")
    mark = key("backspace", False)
    await_action(mark, "Navigate", True)
    # The source row is the sole row in alpha after returning from renamed.
    key("down")
    move("/native/beta/moved")
    moved = snapshot("/native/alpha", {})
    go("/native/beta")
    moved_entry = snapshot("/native/beta", {"/native/beta/moved": "directory"})
    screenshot("files-cross-parent-move")
    go("/native/beta/moved")
    mark = key("backspace", False)
    await_action(mark, "Navigate", True)
    # Removing a non-empty folder is explicitly rejected; then remove its child and parent.
    go("/native/alpha")
    create("populated", "/native/alpha", {"/native/alpha/populated": "directory"})
    go("/native/alpha/populated")
    create("child", "/native/alpha/populated", {"/native/alpha/populated/child": "directory"})
    go("/native/alpha")
    key("down")
    key("delete")
    key("tab")
    mark = key("ret")
    rejected = await_action(mark, "RemoveFolder", False)
    if "NotEmpty" not in rejected["outcome"]:
        raise RuntimeError("non-empty folder removal did not report NotEmpty")
    if "NotEmpty" not in data[mark:].decode(errors="replace"):
        raise RuntimeError("guest serial did not expose the authoritative NotEmpty rejection")
    screenshot("files-nonempty-remove-rejected")
    key("esc")
    go("/native/alpha/populated")
    remove(True)  # child is empty; its source parent is populated
    snapshot("/native/alpha/populated", {})
    go("/native/alpha")
    remove(True)
    snapshot("/native/alpha", {})
    # Remove the moved empty folder and confirm that its old path is absent.
    go("/native/beta")
    remove(True)
    snapshot("/native/beta", {})
    snapshot("/native", {
        **root_files,
        "/native/alpha": "directory",
        "/native/beta": "directory",
    })
    screenshot("files-empty-directories-final")
    for path in ("/native/alpha", "/native/beta"):
        oracle(helper, image, args.store_start, path)
    report["files_directories"] = {"phase": phase, "root_revision": root["revision"],
                                   "nested_listing": nested, "renamed_listing": renamed,
                                   "moved_listing": moved_entry, "actions": actions,
                                   "captures": captures, "timeline": timeline,
                                   "limits": "Bounded explicit folders; empty-folder rename/move/remove; guest UI uses actual QMP PS/2 input; source-bound oracle is read-only."}
    report["checks"].append("actual Files QMP create/navigate/nested create/rename/cross-parent move/NotEmpty reject/empty remove operations agree with paused read-only journal oracle")
    focus_terminal()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    if "--self-test" in sys.argv:
        self_test()
        return 0
    parser.add_argument("--image", type=Path, required=True)
    parser.add_argument("--source-manifest", type=Path, required=True)
    parser.add_argument("--frozen-source-root", type=Path, required=True)
    parser.add_argument("--prepare-record", type=Path)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "build/files-directory-smoke")
    parser.add_argument("--timeout", type=float, default=480)
    parser.add_argument("--cpu", choices=("qemu64", "max"), default="qemu64")
    parser.add_argument("--store-start", type=int, default=131072)
    args = parser.parse_args()
    args.image, args.output_dir = args.image.resolve(), args.output_dir.resolve()
    args.source_manifest, args.frozen_source_root = args.source_manifest.resolve(), args.frozen_source_root.resolve()
    args.prepare_record = args.prepare_record.resolve() if args.prepare_record else None
    if not args.output_dir.is_relative_to(ROOT / "build") or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("output must be beneath build/ and timeout finite positive")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    image_hash = digest(args.image)
    prepare_path = args.prepare_record or args.frozen_source_root.parent / "prepare.json"
    prepare = json.loads(prepare_path.read_text())
    prepare_hash = digest(prepare_path)
    expected_image = prepare.get("sha256", {}).get("next-dev.img")
    if expected_image != image_hash:
        parser.error("input image does not match the immutable frozen prepare record")
    helper, binding = ensure_host(args.source_manifest, args.frozen_source_root, args.output_dir / "host")
    final = {"passed": False, "image_sha256": image_hash,
             "prepare_record_sha256": prepare_hash, "source_binding": binding, "phases": []}
    old_exercise, old_argv = boot_smoke.exercise_file_portal, sys.argv[:]
    try:
        with tempfile.TemporaryDirectory(prefix="files-directories-", dir=ROOT / "work") as temp:
            scratch = Path(temp) / "scratch.img"
            shutil.copyfile(args.image, scratch)
            protected = base.protected_digest(scratch, args.store_start)
            for phase in ("create", "verify"):
                boot_smoke.exercise_file_portal = lambda *hook, phase=phase: exercise(
                    args, phase, scratch, helper, binding, *hook)
                sys.argv = [str(ROOT / "tools/boot_smoke.py"), "--image", str(scratch),
                            "--output-dir", str(args.output_dir / phase), "--timeout", str(args.timeout),
                            "--cpu", args.cpu, "--persistent-data"]
                status = boot_smoke.main()
                boot = json.loads((args.output_dir / phase / "boot-smoke.json").read_text())
                passed = status == 0 and boot.get("passed") is True
                final["phases"].append({"phase": phase, "passed": passed,
                                        "report": str((args.output_dir / phase / "boot-smoke.json").relative_to(ROOT)),
                                        "error": boot.get("error")})
                if not passed:
                    raise RuntimeError(f"{phase} QEMU Files directory acceptance failed: {boot.get('error')}")
                if base.protected_digest(scratch, args.store_start) != protected:
                    raise RuntimeError("guest modified sectors outside authorized store prefix")
            final["passed"] = True
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        final["error"] = str(error)
    finally:
        sys.argv, boot_smoke.exercise_file_portal = old_argv, old_exercise
        final["input_image_unchanged"] = digest(args.image) == image_hash
        final["source_manifest_unchanged"] = digest(args.source_manifest) == binding["manifest_sha256"]
        final["prepare_record_unchanged"] = digest(prepare_path) == prepare_hash
        final["helper_source_unchanged"] = digest(HOST_SOURCE) == binding["helper_source_sha256"]
        final["validator_unchanged"] = digest(ROOT / "tools/files_directory_smoke.py") == binding["validator_sha256"]
        final["retained_sources_unchanged"] = all(
            digest(args.frozen_source_root / relative) == expected
            for relative, expected in binding["sources"].items())
        if not all(final[key] for key in ("input_image_unchanged", "source_manifest_unchanged",
                                          "prepare_record_unchanged",
                                          "helper_source_unchanged", "validator_unchanged",
                                          "retained_sources_unchanged")):
            final["passed"] = False
            final["error"] = "immutable input image or source-bound helper changed during acceptance"
        (args.output_dir / "files-directory.json").write_text(json.dumps(final, indent=2) + "\n")
    print(json.dumps(final, indent=2))
    return 0 if final["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
