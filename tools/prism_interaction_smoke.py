#!/usr/bin/env python3
"""Actual PS/2 Prism interaction and framebuffer evidence on the eighth image.

Extends udp_wait_smoke and the unchanged desktop baseline. Shared geometry and
font comparisons come from a host helper bound to the frozen Prism sources.
"""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

import boot_smoke
import udp_wait_smoke

ROOT = boot_smoke.ROOT
HOST = ROOT / "work/eighth-ui/prism-interaction-host"


def ensure_host():
    """Build only from retained eighth sources, independent of current UI work."""
    manifest = ROOT / "work/eighth-wave/source-manifest.json"
    frozen_root = ROOT / "work/eighth-wave/frozen-src"
    hashes = {path: digest for path, digest in json.loads(manifest.read_text()).items()
              if path.startswith("crates/prism/")}
    for path, expected in hashes.items():
        retained = frozen_root / path
        if not retained.is_file() or hashlib.sha256(retained.read_bytes()).hexdigest() != expected:
            raise RuntimeError(f"retained eighth Prism source/asset differs from frozen manifest: {path}")
    for path in (frozen_root / "crates/prism").rglob("*"):
        if path.is_file() and path.relative_to(frozen_root).as_posix() not in hashes:
            raise RuntimeError(f"unmanifested file in retained Prism crate: {path}")
    probe = ROOT / "tools/prism_interaction_host.rs"
    manifest_hash = hashlib.sha256(manifest.read_bytes()).hexdigest()
    probe_hash = hashlib.sha256(probe.read_bytes()).hexdigest()
    binding_path = HOST.parent / "host-binding.json"
    library = HOST.parent / "libprism.rlib"
    if HOST.is_file() and binding_path.is_file():
        binding = json.loads(binding_path.read_text())
        if (binding.get("frozen_source_root") == str(frozen_root.relative_to(ROOT))
                and binding.get("compiled_source_manifest_sha256") == manifest_hash
                and binding.get("probe_source_sha256") == probe_hash
                and binding.get("prism_sources") == hashes
                and library.is_file()
                and binding.get("library_sha256") == hashlib.sha256(library.read_bytes()).hexdigest()
                and binding.get("helper_sha256") == hashlib.sha256(HOST.read_bytes()).hexdigest()):
            return binding
    rustc = shutil.which("rustc")
    if not rustc:
        raise RuntimeError("rustc unavailable; source tools/env.sh before running this gate")
    HOST.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run([rustc, "--edition", "2021", "--crate-name", "prism", "--crate-type", "rlib",
                    "-Cpanic=abort", str(frozen_root / "crates/prism/src/lib.rs"),
                    "-o", str(library)], check=True)
    subprocess.run([rustc, "--edition", "2021", "-Cpanic=abort", str(probe),
                    "--extern", f"prism={library}", "-o", str(HOST)], check=True)
    binding = {"compiled_source_manifest_sha256": manifest_hash, "prism_sources": hashes,
               "frozen_source_root": str(frozen_root.relative_to(ROOT)),
               "helper_sha256": hashlib.sha256(HOST.read_bytes()).hexdigest(),
               "library_sha256": hashlib.sha256(library.read_bytes()).hexdigest(),
               "probe_source_sha256": probe_hash, "panic_strategy": "abort"}
    binding_path.write_text(json.dumps(binding, indent=2) + "\n")
    return binding


def ppm(data):
    header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
    if not header:
        raise RuntimeError("invalid framebuffer PPM")
    width, height = int(header[1]), int(header[2])
    pixels = data[header.end():]
    if len(pixels) != width * height * 3:
        raise RuntimeError("invalid framebuffer length")
    return width, height, pixels


def crop(image, bounds):
    width, height, pixels = image
    x, y, w, h = bounds
    if min(x, y, w, h) < 0 or x + w > width or y + h > height:
        raise RuntimeError("shared geometry outside captured framebuffer")
    return b"".join(pixels[((y + row) * width + x) * 3:
                           ((y + row) * width + x + w) * 3] for row in range(h))


def text_matches(image, x, y, text, style="body"):
    template = ppm(subprocess.check_output([str(HOST), style, text]))
    actual = crop(image, (x, y, template[0], template[1]))
    background = bytes.fromhex("e2edff" if style == "muted_selected" else "f7f9fc")
    foreground = [index for index in range(0, len(actual), 3)
                  if template[2][index:index + 3] != background]
    return bool(foreground) and all(actual[index:index + 3] == template[2][index:index + 3]
                                    for index in foreground)


def exercise_prism(serial, data, wait_text, wait_regex, wait_exit,
                   qmp, remaining, report, original):
    binding = ensure_host()
    frozen = ROOT / "work/eighth-wave/source-manifest.json"
    if hashlib.sha256(frozen.read_bytes()).hexdigest() != binding["compiled_source_manifest_sha256"]:
        raise RuntimeError("host geometry belongs to a different compiled-source snapshot")
    if hashlib.sha256(HOST.read_bytes()).hexdigest() != binding["helper_sha256"]:
        raise RuntimeError("shared geometry/font helper changed after binding")
    dimensions = wait_regex(rb"PRISM: READY (\d+)x(\d+);")
    width, height = int(dimensions[1]), int(dimensions[2])
    geometry = {}
    for line in subprocess.check_output([str(HOST), "geometry", str(width), str(height)]).decode().splitlines():
        name, *bounds = line.split()
        geometry[name] = tuple(map(int, bounds))
    args = sys.argv[1:]
    output = ROOT / "build/prism-interaction"
    for index, argument in enumerate(args):
        if argument == "--output-dir":
            output = Path(args[index + 1]).resolve()
        elif argument.startswith("--output-dir="):
            output = Path(argument.split("=", 1)[1]).resolve()
    evidence = output / "prism-interaction"
    evidence.mkdir(parents=True, exist_ok=True)
    captures = []

    def screenshot(name):
        path = evidence / (f"{len(captures):02d}-" + name + ".ppm")
        qmp("screendump", {"filename": str(path), "format": "ppm"})
        raw = path.read_bytes()
        captures.append({"name": name, "path": str(path.relative_to(ROOT)),
                         "sha256": hashlib.sha256(raw).hexdigest()})
        return ppm(raw)

    def key(code, barrier="left"):
        start = len(data)
        qmp("send-key", {"keys": [{"type": "qcode", "data": code}], "hold-time": 40})
        if code == "f8":
            # The frozen image renders Notifications but does not emit FRAME
            # for it. An ordered PS/2 no-op with a logged frame is the barrier.
            qmp("send-key", {"keys": [{"type": "qcode", "data": barrier}], "hold-time": 40})
        wait_text("PRISM FRAME:", start)
        time.sleep(min(0.08, remaining()))

    position = [width // 2, height // 2]

    def click(bounds):
        target = [bounds[0] + bounds[2] // 2, bounds[1] + bounds[3] // 2]
        while position != target:
            movement = [max(-100, min(100, target[i] - position[i])) for i in range(2)]
            events = [{"type": "rel", "data": {"axis": axis, "value": movement[i]}}
                      for i, axis in enumerate(("x", "y")) if movement[i]]
            qmp("input-send-event", {"events": events})
            position[:] = [position[i] + movement[i] for i in range(2)]
            time.sleep(min(0.02, remaining()))
        start = len(data)
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"down": True, "button": "left"}}]})
        event = wait_regex(rb"PRISM POINTER: x=(\d+) y=(\d+) buttons=1 focused=([^\r\n]+)", start)
        if [int(event[1]), int(event[2])] != target:
            raise RuntimeError("PS/2 pointer did not reach shared hit geometry")
        released = len(data)
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"down": False, "button": "left"}}]})
        wait_text("buttons=0", released)
        end = data.index(b"buttons=0", released) + len(b"buttons=0")
        wait_text("PRISM FRAME:", end)
        return event[3]

    click(geometry["home_button"])
    home = screenshot("home-pointer")
    hx, hy, _, _ = geometry["home"]
    if not text_matches(home, hx + 28, hy + 18, "Home", "title"):
        raise RuntimeError("taskbar Home click did not render the shared Home panel")
    if click(geometry["home_files"]) != b"Some(Files)":
        raise RuntimeError("Home Files action did not focus the real Files window")
    screenshot("files-pointer")
    if click(geometry["settings_button"]) != b"Some(Settings)":
        raise RuntimeError("taskbar Settings click did not focus the real Settings window")
    screenshot("settings-pointer")
    if click(geometry["terminal_button"]) != b"Some(Terminal)":
        raise RuntimeError("taskbar Terminal click did not restore terminal focus")
    report["checks"].append("actual PS/2 centered taskbar Home click and Home Files launch use shared resolution-specific geometry")

    nx, ny, _, _ = geometry["notices"]

    def counts(image):
        matches = [(unread, total) for total in range(9) for unread in range(total + 1)
                   if text_matches(image, nx + 12, ny + 16,
                                   f"NOTICES {unread} unread / {total} stored")]
        if len(matches) != 1:
            raise RuntimeError("notification count raster did not match one shared-font state")
        return matches[0]

    key("f8")
    notices = screenshot("notices-unread")
    initial = counts(notices)
    if initial[0] == 0:
        raise RuntimeError("no real unread process notification to exercise")
    accepted = list(re.finditer(
        rb"PRISM: process notification pid=(\d+) exit=(-?\d+) accepted=true\r?\n", data))
    source_pid, source_exit = int(accepted[-1][1]), int(accepted[-1][2])
    identity = wait_regex(rb"HELIX: APPLICATION IDENTITY pid=" + str(source_pid).encode()
                          + rb" package=([^ ]+) signer=", 0)
    label = identity[1].decode("ascii")
    rows = max(0, min(8, (geometry["notices"][3] - 220) // 64))
    origin = f"Process PID {source_pid} / {'Info' if source_exit == 0 else 'Warning'}"
    for _ in range(8):
        if text_matches(notices, nx + 12, ny + 80 + rows * 64, origin):
            break
        key("down")
        notices = screenshot("notices-source-select")
    else:
        raise RuntimeError("actual signed process source does not appear in notification details")
    columns = (geometry["notices"][2] - 24) // (12 if width >= 800 else 6)
    for index, start in enumerate(range(0, len(label), columns)):
        if not text_matches(notices, nx + 12, ny + 72 + 24 + index * 18,
                            label[start:start + columns], "muted_selected"):
            raise RuntimeError("selected notification label differs from actual authenticated process identity")
    key("ret")
    read = counts(screenshot("notices-read"))
    if read != (initial[0] - 1, initial[1]):
        raise RuntimeError("notification Enter did not mark one selected event read")
    key("tab")
    key("ret")
    dismissed = counts(screenshot("notices-dismissed"))
    if dismissed != (read[0], read[1] - 1):
        raise RuntimeError("notification dismiss did not remove the selected read event")
    if text_matches(screenshot("notices-source-dismissed"), nx + 12,
                    ny + 80 + rows * 64, origin):
        raise RuntimeError("dismissed authenticated process source remained selected")
    key("tab")
    key("tab")
    key("ret")
    dnd = screenshot("notices-dnd-on")
    if not text_matches(dnd, nx + 12, ny + 42, "Do Not Disturb: ON (session)", "muted"):
        raise RuntimeError("notification Do Not Disturb did not become ON")
    key("f8")
    key("f8")
    reopened = screenshot("notices-dnd-persisted")
    if not text_matches(reopened, nx + 12, ny + 42, "Do Not Disturb: ON (session)", "muted"):
        raise RuntimeError("Do Not Disturb did not persist across center reopen")
    for _ in range(3):
        key("tab")
    key("ret")
    restored = screenshot("notices-dnd-off")
    if not text_matches(restored, nx + 12, ny + 42, "Do Not Disturb: OFF (session)", "muted"):
        raise RuntimeError("Do Not Disturb did not restore OFF")
    key("f8")
    report["checks"].append("actual F8 notification center read, dismiss and persistent session DND transitions match shared font rasters")
    guarded = False

    def modal_qmp(command, arguments=None):
        nonlocal guarded
        keys = arguments.get("keys", []) if arguments else []
        if not guarded and command == "send-key" and any(item.get("data") == "down" for item in keys):
            guarded = True
            # Left toggles the chooser's approval choice in the frozen handler;
            # Menu/F10 is its no-op and still receives a logged frame.
            # PORTAL is logged before presentation. Complete a Menu frame first,
            # otherwise the baseline can still contain only the Terminal.
            key("f10")
            before = screenshot("consent-before-f8")
            px, py, _, _ = geometry["portal"]
            if not text_matches(before, px + 16, py + 16, "NEXT / TRUSTED FILE ACCESS"):
                raise RuntimeError("consent baseline was captured before the chooser completed presentation")
            if not text_matches(before, px + 16, py + 42,
                                "org.next.development.native.portal-probe"):
                raise RuntimeError("consent baseline does not show the authenticated portal caller")
            key("f8", barrier="f10")
            after = screenshot("consent-after-f8")
            if crop(before, geometry["portal"]) != crop(after, geometry["portal"]):
                raise RuntimeError("F8 displaced or mutated the security-sensitive file consent modal")
            report["checks"].append("actual F8 is ignored by the active file consent modal; exact modal framebuffer preserved")
        return qmp(command, arguments)

    original(serial, data, wait_text, wait_regex, wait_exit, modal_qmp, remaining, report)
    if not guarded:
        raise RuntimeError("file consent modal priority was not exercised")
    report["prism_interaction"] = {
        "host_helper_sha256": binding["helper_sha256"],
        "compiled_source_manifest_sha256": binding["compiled_source_manifest_sha256"],
        "frozen_source_root": binding["frozen_source_root"],
        "frozen_prism_library_sha256": binding["library_sha256"],
        "viewport": [width, height], "geometry": geometry,
        "notification_counts": {"initial": initial, "read": read, "dismissed": dismissed},
        "authenticated_notification": {"pid": source_pid, "package_id": label,
                                       "exit_code": source_exit, "read_then_dismissed": True},
        "captures": captures,
        "limits": "Guest proves existing process notices and consent-modal priority. Critical-priority event injection is not exposed by this image; that DND exception remains a host-model test.",
    }


def main():
    if "--help" not in sys.argv and "-h" not in sys.argv:
        ensure_host()
    if not any(arg == "--output-dir" or arg.startswith("--output-dir=") for arg in sys.argv):
        sys.argv.extend(["--output-dir", str(ROOT / "build/prism-interaction")])
    original = boot_smoke.exercise_file_portal
    boot_smoke.exercise_file_portal = lambda *args: exercise_prism(*args, original)
    try:
        return udp_wait_smoke.main()
    finally:
        boot_smoke.exercise_file_portal = original


if __name__ == "__main__":
    sys.exit(main())
