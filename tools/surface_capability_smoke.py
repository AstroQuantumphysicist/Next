#!/usr/bin/env python3
"""Observe native capability-backed copied pixels in the actual development UI.

Reuses DNS and desktop baseline boot/cleanup; no host-rendered picture substitutes
for the guest screenshot. Presentation remains inside the trusted kernel Prism.
"""
import hashlib
import json
from pathlib import Path
import re
import sys

import boot_smoke
import dns_capability_smoke

ROOT = boot_smoke.ROOT
PACKAGE = "org.next.development.native.surface-probe"
MARKER = "SURFACE PROBE: authenticated copied BGRX quadrants, configure/ack, rights, stale handles, quotas and publication lifecycle passed"
COLORS = tuple(bytes.fromhex(color) for color in ("2070e0", "f0d020", "20c0b0", "a040d0"))
BINDING = None


def ppm(raw):
    match = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", raw)
    if not match:
        raise RuntimeError("guest screenshot is not binary RGB PPM")
    width, height = map(int, match.groups())
    pixels = raw[match.end():]
    if len(pixels) != width * height * 3:
        raise RuntimeError("guest screenshot has a truncated pixel extent")
    return width, height, pixels


def quadrants(image):
    """Locate complete literal quadrants, including fractional nearest scaling."""
    width, height, pixels = image
    offset = pixels.find(COLORS[0] * 32)
    while offset >= 0:
        pixel = offset // 3
        x, y = pixel % width, pixel // width
        if offset % 3 or (x and pixels[offset - 3:offset] == COLORS[0]):
            offset = pixels.find(COLORS[0] * 32, offset + 3)
            continue
        halves = []
        cursor = offset
        for color in COLORS[:2]:
            length = 0
            while x + sum(halves) + length < width and pixels[cursor:cursor + 3] == color:
                length += 1
                cursor += 3
            halves.append(length)
        w = sum(halves)
        if min(halves) >= 32 and abs(halves[0] - halves[1]) <= 1 and w <= 512:
            top = COLORS[0] * halves[0] + COLORS[1] * halves[1]
            bottom = COLORS[2] * halves[0] + COLORS[3] * halves[1]
            counts = []
            row = y
            for expected in (top, bottom):
                count = 0
                while row < height:
                    begin = (row * width + x) * 3
                    if pixels[begin:begin + w * 3] != expected:
                        break
                    count += 1
                    row += 1
                counts.append(count)
            h = sum(counts)
            if min(counts) >= 24 and abs(counts[0] - counts[1]) <= 1 and abs(w * 48 - h * 64) < 64:
                return {"x": x, "y": y, "width": w, "height": h,
                        "scale_x": w / 64, "scale_y": h / 48}
        offset = pixels.find(COLORS[0] * 32, offset + 3)
    return None


def exercise_surface(serial, data, wait_text, wait_regex, wait_exit,
                     qmp, remaining, report, original):
    start = len(data)
    serial.sendall(b"run /system/surface-probe.elf\r")
    spawned = wait_regex(rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/surface-probe\.elf profile=Native", start)
    pid = int(spawned[1])
    identity = wait_regex(rb"HELIX: APPLICATION IDENTITY pid=" + str(pid).encode()
                          + rb" package=([^ ]+) signer=", start)
    if identity[1].decode() != PACKAGE:
        raise RuntimeError("surface fixture package identity differs from its signed principal")
    wait_text(f"NEXT APP pid={pid}: {MARKER}", start)
    pressure = wait_regex(rb"NEXT APP pid=" + str(pid).encode()
                          + rb": SURFACE PRESSURE: (observed WouldBlock|all32 releases completed between calls)\r?\n", start)
    completed = wait_regex(rb"PRISM SURFACE: slot=(\d+) generation=(\d+) frame=(\d+) pid="
                           + str(pid).encode() + rb" package=([^\r\n ]+)\r?\n", start)
    if completed[4].decode() != PACKAGE or int(completed[3]) != 1:
        raise RuntimeError("completed client frame has the wrong authenticated origin or generation")
    if re.search(rb"HELIX: PROCESS EXIT pid=" + str(pid).encode() + rb" ", data[start:]):
        raise RuntimeError("surface fixture exited before the observed presentation screenshot")
    output = ROOT / "build/surface-capability"
    for index, argument in enumerate(sys.argv[1:]):
        if argument == "--output-dir":
            output = Path(sys.argv[index + 2]).resolve()
        elif argument.startswith("--output-dir="):
            output = Path(argument.split("=", 1)[1]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    shown = output / "surface-presented.ppm"
    qmp("screendump", {"filename": str(shown), "format": "ppm"})
    raw = shown.read_bytes()
    bounds = quadrants(ppm(raw))
    if bounds is None:
        raise RuntimeError("actual guest framebuffer does not contain all copied BGRX quadrant pixels")
    # No child scheduling count substitutes for the completed frame evidence.
    exited = wait_exit(pid, 49, start)
    cleared = wait_regex(rb"PRISM SURFACE CLEARED: slot=" + completed[1]
                         + rb" generation=" + completed[2] + rb" pid=" + str(pid).encode()
                         + rb"\r?\n", exited.start())
    removed = output / "surface-cleared.ppm"
    qmp("screendump", {"filename": str(removed), "format": "ppm"})
    removed_raw = removed.read_bytes()
    if quadrants(ppm(removed_raw)) is not None:
        raise RuntimeError("closed surface still occupies the completed guest framebuffer")
    if re.search(rb"HELIX: PROCESS FAULT pid=" + str(pid).encode() + rb" ", data[start:]):
        raise RuntimeError("surface fixture faulted instead of completing capability checks")
    final = len(data)
    serial.sendall(b"ps\r")
    wait_text("NEXT PROCESS: pid=1", final)
    end = data.index(b"NEXT PROCESS: pid=1", final) + len(b"NEXT PROCESS: pid=1")
    wait_text("PRISM FRAME:", end)
    if re.search(rb"NEXT PROCESS: pid=" + str(pid).encode() + rb" ", data[final:]):
        raise RuntimeError("surface fixture process was not collected")
    report["native_surface"] = {
        "pid": pid, "package_id": PACKAGE, "exit_code": 49,
        "surface_slot": int(completed[1]), "surface_generation": int(completed[2]),
        "frame_generation": int(completed[3]), "literal_pixel_bounds": bounds,
        "presentation_precedes_exit": completed.start() < exited.start(),
        "cleanup_after_exit": cleared.start() > exited.start(),
        "publication_pressure": pressure[1].decode(),
        "actual_would_block_observed": pressure[1] == b"observed WouldBlock",
        "captures": [{"path": str(path.relative_to(ROOT)), "sha256": hashlib.sha256(content).hexdigest()}
                     for path, content in ((shown, raw), (removed, removed_raw))],
        "limits": "CPU-copy application preview in trusted kernel-hosted Prism; no isolated compositor, GPU support, protected capture or app input delivery",
    }
    report["surface_source_binding"] = dict(BINDING)
    report["checks"].append("signed native surface capability fixture completes configure/ack, copyout, rights, stale/role/buffer/quota and publication lifecycle checks")
    report["checks"].append("completed guest framebuffer contains all literal copied app pixels before exit and removes them after teardown")
    return original(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report)


def self_test():
    for w, h in ((64, 48), (128, 96), (256, 192), (248, 186), (247, 185)):
        width, height = w + 8, h + 8
        rows = [b"\0" * width * 3 for _ in range(4)]
        left = (w + 1) // 2
        for y in range(h):
            pair = COLORS[:2] if y < (h + 1) // 2 else COLORS[2:]
            rows.append(b"\0" * 12 + pair[0] * left + pair[1] * (w - left) + b"\0" * 12)
        rows.extend([b"\0" * width * 3] * 4)
        pixels = b"".join(rows)
        assert quadrants((width, height, pixels)) == {
            "x": 4, "y": 4, "width": w, "height": h,
            "scale_x": w / 64, "scale_y": h / 48}
        corrupt = bytearray(pixels)
        corrupt[((4 + h // 2 + 1) * width + 4 + w // 2 + 1) * 3] ^= 1
        assert quadrants((width, height, bytes(corrupt))) is None
    print("Literal guest quadrant detector self-test PASS; no guest boot performed")


def main():
    if "--self-test" in sys.argv:
        return self_test()
    if "--help" in sys.argv or "-h" in sys.argv:
        print("Surface gate also requires --source-manifest PATH --source-manifest-sha256 SHA256 --expected-image-sha256 SHA256")
        return dns_capability_smoke.main()
    # Bind the corrected retained snapshot, never mutable concurrent sources.
    custom = {}
    forwarded = [sys.argv[0]]
    index = 1
    while index < len(sys.argv):
        argument = sys.argv[index]
        key = argument.split("=", 1)[0]
        if key in ("--source-manifest", "--source-manifest-sha256", "--expected-image-sha256"):
            if "=" in argument:
                custom[key] = argument.split("=", 1)[1]
            else:
                index += 1
                if index >= len(sys.argv):
                    raise SystemExit(f"{key} requires a value")
                custom[key] = sys.argv[index]
        else:
            forwarded.append(argument)
        index += 1
    if len(custom) != 3:
        raise SystemExit("Surface gate requires the retained source manifest and expected manifest/image SHA256 values")
    manifest = Path(custom["--source-manifest"]).resolve()
    manifest_hash = hashlib.sha256(manifest.read_bytes()).hexdigest()
    if manifest_hash != custom["--source-manifest-sha256"]:
        raise SystemExit("Retained corrected source manifest SHA256 differs from expected value")
    sources = json.loads(manifest.read_text())
    fixture = "crates/next-user/src/bin/surface-probe.rs"
    if fixture not in sources:
        raise SystemExit("Retained manifest does not bind the actual surface fixture source")
    image = ROOT / "build/next-dev.img"
    for index, argument in enumerate(forwarded[1:]):
        if argument == "--image":
            image = Path(forwarded[index + 2]).resolve()
        elif argument.startswith("--image="):
            image = Path(argument.split("=", 1)[1]).resolve()
    expected_image = custom["--expected-image-sha256"]
    if hashlib.sha256(image.read_bytes()).hexdigest() != expected_image:
        raise SystemExit("Loaded image SHA256 differs from the expected corrected-source image")
    global BINDING
    BINDING = {"manifest_path": str(manifest), "manifest_sha256": manifest_hash,
               "image_sha256": expected_image, "surface_fixture_sha256": sources[fixture],
               "workspace_policy": "Mutable concurrent source is not substituted for retained compiled evidence"}
    sys.argv[:] = forwarded
    if not any(arg == "--output-dir" or arg.startswith("--output-dir=") for arg in sys.argv):
        sys.argv.extend(["--output-dir", str(ROOT / "build/surface-capability")])
    original = boot_smoke.exercise_file_portal
    boot_smoke.exercise_file_portal = lambda *args: exercise_surface(*args, original)
    try:
        result = dns_capability_smoke.main()
        manifest_unchanged = hashlib.sha256(manifest.read_bytes()).hexdigest() == manifest_hash
        image_unchanged = hashlib.sha256(image.read_bytes()).hexdigest() == expected_image
        output = ROOT / "build/surface-capability"
        for index, argument in enumerate(sys.argv[1:]):
            if argument == "--output-dir":
                output = Path(sys.argv[index + 2]).resolve()
            elif argument.startswith("--output-dir="):
                output = Path(argument.split("=", 1)[1]).resolve()
        report_path = output / "boot-smoke.json"
        if report_path.is_file():
            report = json.loads(report_path.read_text())
            report["surface_source_binding"] = dict(BINDING,
                manifest_unchanged=manifest_unchanged, image_unchanged=image_unchanged)
            if not manifest_unchanged or not image_unchanged:
                report["passed"] = False
                report["error"] = "Retained compiled-source manifest or loaded image changed during the surface gate"
                result = 1
            report_path.write_text(json.dumps(report, indent=2) + "\n")
        elif not manifest_unchanged or not image_unchanged:
            raise RuntimeError("Retained source binding or loaded image changed during the surface gate")
        return result
    finally:
        boot_smoke.exercise_file_portal = original


if __name__ == "__main__":
    sys.exit(main())
