#!/usr/bin/env python3
"""Observe gesture-routed capability input and actual owned response pixels.

The retained source/image binding is shared with the surface gate. No host input
queue or rendered reference replaces the guest's signed processes and display.
"""
import hashlib
import json
from pathlib import Path
import re
import sys
import time
import surface_capability_smoke as surface

PACKAGE = "org.next.development.native.client-input-probe"
THREE_BUTTON = False


def solid_blocks(image):
    width, _, pixels = image
    blocks = []
    for rgb in ("3060b0", "2070e0", "30d030", "f0c020", "4080c0", "707070", "a050a0"):
        color = bytes.fromhex(rgb)
        index = pixels.find(color * 64)
        while index >= 0:
            x, y = (index // 3) % width, (index // 3) // width
            if index % 3 == 0 and (not x or pixels[index - 3:index] != color):
                if (not y or pixels[index - width * 3:index - width * 3 + 3] != color) and (x, y) not in blocks:
                    run = 0
                    while x + run < width and pixels[index + run * 3:index + (run + 1) * 3] == color:
                        run += 1
                    vertical = 0
                    while y + vertical < len(pixels) // (width * 3):
                        begin = index + vertical * width * 3
                        if pixels[begin:begin + run * 3] != color * run:
                            break
                        vertical += 1
                    if 64 <= run <= 512 and vertical >= 48:
                        blocks.append((x, y))
            index = pixels.find(color * 64, index + 3)
    blocks.sort(key=lambda position: (position[1], position[0]))
    return blocks


def exercise(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report, original):
    binding = surface.BINDING
    manifest = json.loads(Path(binding["manifest_path"]).read_text())
    fixture = "crates/next-user/src/bin/client-input-probe.rs"
    if fixture not in manifest:
        raise RuntimeError("retained manifest does not bind the actual client-input fixture")
    output = surface.ROOT / "build/client-input"
    for index, argument in enumerate(sys.argv[1:]):
        if argument == "--output-dir":
            output = Path(sys.argv[index + 2]).resolve()
    output.mkdir(parents=True, exist_ok=True)
    captures = []
    clients = []
    button_packets = []
    if THREE_BUTTON:
        retained = Path(binding["manifest_path"]).parent / "frozen-src"
        required = {
            "crates/helix-kernel/src/main.rs": ("on_pointer_packet", "PRISM POINTER PRESENTED"),
            "crates/helix-kernel/src/input.rs": ("poll_with", "serial_first"),
            "crates/next-user/src/client_input_fixture.rs": ("publication_delay_ns",),
            "crates/next-user/src/bin/client-input-delayed.rs": ("2000000000",),
        }
        for name, markers in required.items():
            path = retained / name
            if name not in manifest or hashlib.sha256(path.read_bytes()).hexdigest() != manifest[name]:
                raise RuntimeError("three-button proof requires exact retained Source13 input/fixture sources")
            if any(marker not in path.read_text() for marker in markers):
                raise RuntimeError("three-button proof was requested against an older input producer")

    def completed_launch(client):
        wait_text(f"NEXT APP pid={client['pid']}: CLIENT INPUT PROBE: ready; creation/publication did not focus; READ/WAIT attenuation passed", client["start"])
        frame = wait_regex(rb"PRISM SURFACE: slot=(\d+) generation=(\d+) frame=1 pid=" + str(client["pid"]).encode() + rb" package=" + client["package"].encode() + rb"\r?\n", client["start"])
        client.update(slot=int(frame[1]), generation=int(frame[2]))
        if re.search(rb"PRISM CLIENT FOCUS: .*pid=" + str(client["pid"]).encode() + rb" accepted=true", data[client["start"]:frame.end()]):
            raise RuntimeError("publication focused a client without a user gesture")
        return frame

    def launch(delayed=False):
        name = "client-input-delayed" if delayed else "client-input-probe"
        package = f"org.next.development.native.{name}"
        start = len(data)
        serial.sendall(f"run /system/{name}.elf\r".encode())
        spawned = wait_regex(rb"HELIX: PROCESS SPAWN pid=(\d+) slot=\d+ path=/system/" + name.encode() + rb"\.elf profile=Native", start)
        pid = int(spawned[1])
        identity = wait_regex(rb"HELIX: APPLICATION IDENTITY pid=" + str(pid).encode() + rb" package=([^ ]+) signer=", start)
        if identity[1].decode() != package:
            raise RuntimeError("client input fixture has an unexpected authenticated principal")
        client = {"pid": pid, "package": package, "start": start}
        clients.append(client)
        if delayed:
            wait_text(f"NEXT APP pid={pid}: CLIENT INPUT PROBE: bounded publication delay started; no focus requested", start)
        else:
            completed_launch(client)
        return client

    def key(code):
        qmp("send-key", {"keys": [{"type": "qcode", "data": code}], "hold-time": 40})

    def capture(name, bounds, color=None):
        path = output / f"client-{name}.ppm"
        qmp("screendump", {"filename": str(path), "format": "ppm"})
        raw = path.read_bytes()
        width, height, pixels = surface.ppm(raw)
        if color is not None:
            x, y, w, h = bounds
            # Top-left content sample avoids the pointer resting at the center.
            if x + w > width or y + h > height or w < 16 or h < 16:
                raise RuntimeError("trusted content geometry is outside the guest display")
            expected = bytes.fromhex(color) * 8
            for row in range(y + 3, y + 11):
                offset = (row * width + x + 3) * 3
                if pixels[offset:offset + 24] != expected:
                    raise RuntimeError("owned response frame does not contain its literal app color")
        captures.append({"path": str(path.relative_to(surface.ROOT)), "sha256": hashlib.sha256(raw).hexdigest()})

    def motion(dx, dy):
        start = len(data)
        events = [{"type": "rel", "data": {"axis": axis, "value": value}}
                  for axis, value in (("x", dx), ("y", dy)) if value]
        # Frozen11 only logs pointer/frame on button transitions. A real middle
        # transition provides coordinates without invoking a left-focus gesture.
        if THREE_BUTTON:
            qmp("input-send-event", {"events": events})
            event = wait_regex(rb"PRISM POINTER PRESENTED: generation=\d+ x=(\d+) y=(\d+) buttons=0\r?\n", start)
            return list(map(int, event.groups()))
        events.append({"type": "btn", "data": {"button": "middle", "down": True}})
        qmp("input-send-event", {"events": events})
        pointed = wait_regex(rb"PRISM POINTER: x=(\d+) y=(\d+) buttons=4 focused=[^\r\n]+\r?\n", start)
        wait_text("PRISM FRAME:", pointed.end())
        released = len(data)
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"button": "middle", "down": False}}]})
        event = wait_regex(rb"PRISM POINTER: x=(\d+) y=(\d+) buttons=0 focused=[^\r\n]+\r?\n", released)
        wait_text("PRISM FRAME:", event.end())
        return list(map(int, event.groups()))

    def focus(client):
        for _ in range(20):
            position = motion(-100, -100)
            if position == [0, 0]:
                break
        else:
            raise RuntimeError("actual PS/2 pointer failed to reach the clamped corner")
        # Source11 root logs content on successful focus; initial card positions
        # are read from actual framebuffer color blocks, never guessed geometry.
        path = output / "client-locate.ppm"
        qmp("screendump", {"filename": str(path), "format": "ppm"})
        width, _, pixels = surface.ppm(path.read_bytes())
        blocks = solid_blocks((width, 0, pixels))
        if len(blocks) != len(clients):
            raise RuntimeError("cannot identify all actual owned preview content blocks")
        target = [blocks[client["slot"]][0] + 16, blocks[client["slot"]][1] + 16]
        for _ in range(24):
            if position == target:
                break
            step = [max(-100, min(100, target[i] - position[i])) for i in range(2)]
            previous = position
            position = motion(*step)
            if previous == position:
                raise RuntimeError("actual PS/2 motion made no progress toward client content")
        else:
            raise RuntimeError("actual PS/2 pointer failed to reach the client content")
        start = len(data)
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"button": "left", "down": True}}]})
        pointed = wait_regex(rb"PRISM POINTER: x=(\d+) y=(\d+) buttons=1 focused=[^\r\n]+", start)
        if list(map(int, pointed.groups())) != target:
            raise RuntimeError(f"actual PS/2 pointer {pointed.groups()} differs from requested target {target}")
        wait_text(f"PRISM CLIENT FOCUS: slot={client['slot']} generation={client['generation']} pid={client['pid']} accepted=true", start)
        geometry = wait_regex(rb"PRISM CLIENT CONTENT: slot=" + str(client["slot"]).encode() + rb" generation=" + str(client["generation"]).encode() + rb" pid=" + str(client["pid"]).encode() + rb" x=(\d+) y=(\d+) width=(\d+) height=(\d+)", start)
        client["bounds"] = tuple(map(int, geometry.groups()))
        released = len(data)
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"button": "left", "down": False}}]})
        wait_regex(rb"PRISM POINTER: x=\d+ y=\d+ buttons=0 focused=[^\r\n]+", released)
        wait_text(f"NEXT APP pid={client['pid']}: CLIENT INPUT PROBE: short/null/readonly/guard copyout preserved head event and level readiness", client["start"])

    def refocus_here(client):
        start = len(data)
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"button": "left", "down": True}}]})
        event = wait_regex(rb"PRISM POINTER PRESENTED: generation=\d+ x=(\d+) y=(\d+) buttons=1\r?\n", start)
        x, y = map(int, event.groups())
        bx, by, bw, bh = client["bounds"]
        if not (bx <= x < bx + bw and by <= y < by + bh):
            raise RuntimeError("refocus gesture no longer targets the first owned client")
        wait_text(f"PRISM CLIENT FOCUS: slot={client['slot']} generation={client['generation']} pid={client['pid']} accepted=true", start)
        released = len(data)
        qmp("input-send-event", {"events": [{"type": "btn", "data": {"button": "left", "down": False}}]})
        wait_regex(rb"PRISM POINTER PRESENTED: generation=\d+ x=\d+ y=\d+ buttons=0\r?\n", released)
        return start

    def button_packet(client, mask, require_delivery=True):
        start = len(data)
        events = [{"type": "btn", "data": {"button": button, "down": bool(mask & bit)}}
                  for button, bit in (("left", 1), ("right", 2), ("middle", 4))]
        events.append({"type": "rel", "data": {"axis": "x", "value": 1}})
        qmp("input-send-event", {"events": events})
        presented = wait_regex(rb"PRISM POINTER PRESENTED: generation=(\d+) x=(\d+) y=(\d+) buttons=" + str(mask).encode() + rb"\r?\n", start)
        if require_delivery:
            event = wait_regex(rb"NEXT APP pid=" + str(client["pid"]).encode() + rb": CLIENT INPUT EVENT: seq=(\d+) configure=\d+ kind=4 x=(\d+) y=(\d+) buttons=" + str(mask).encode() + rb" scalar=0 timestamp=(\d+)\r?\n", start)
            completed = wait_regex(rb"PRISM SURFACE: slot=" + str(client["slot"]).encode() + rb" generation=" + str(client["generation"]).encode() + rb" frame=" + str(int(event[1]) + 1).encode() + rb" pid=" + str(client["pid"]).encode() + rb" package=" + client["package"].encode() + rb"\r?\n", event.start())
            records = re.findall(rb"NEXT APP pid=" + str(client["pid"]).encode() + rb": CLIENT INPUT EVENT: seq=\d+ configure=\d+ kind=4 [^\r\n]+", data[start:completed.end()])
            if len(records) != 1:
                raise RuntimeError("one physical packet produced multiple/transient client pointer events")
            if re.search(rb"PRISM CLIENT FOCUS: .*accepted=true|CLIENT INPUT EVENT: .*kind=1 ", data[start:completed.end()]):
                raise RuntimeError("same-client left/chord transition acquired focus again")
        elif re.search(rb"PRISM CLIENT FOCUS: .*accepted=true|NEXT APP pid=\d+: CLIENT INPUT EVENT:", data[start:presented.end()]):
            raise RuntimeError("background right/middle packet acquired client focus or input")
        button_packets.append({"mask": mask, "delivery_expected": require_delivery,
                               "presented_generation": int(presented[1]), "serial_start": start})

    def character(client, code, color):
        start = len(data)
        key(code)
        event = wait_regex(rb"NEXT APP pid=" + str(client["pid"]).encode() + rb": CLIENT INPUT EVENT: seq=(\d+) configure=\d+ kind=3 x=0 y=0 buttons=0 scalar=" + str(ord(code)).encode() + rb" timestamp=(\d+)", start)
        frame = wait_regex(rb"PRISM SURFACE: slot=" + str(client["slot"]).encode() + rb" generation=" + str(client["generation"]).encode() + rb" frame=" + str(int(event[1]) + 1).encode() + rb" pid=" + str(client["pid"]).encode() + rb" package=" + client["package"].encode() + rb"\r?\n", event.start())
        for other in clients:
            if other is not client and re.search(rb"NEXT APP pid=" + str(other["pid"]).encode() + rb": CLIENT INPUT EVENT: .*kind=3 .*scalar=" + str(ord(code)).encode() + rb" ", data[start:]):
                raise RuntimeError("semantic key reached a background client")
        capture(f"pid-{client['pid']}-{code}", client["bounds"], color)
        return frame.end()

    first = launch()
    if THREE_BUTTON:
        # Initial pointer preparation cannot activate a client. Locate its actual
        # content, then test non-left masks before the first left focus gesture.
        for _ in range(20):
            position = motion(-100, -100)
            if position == [0, 0]: break
        else:
            raise RuntimeError("background pointer preparation failed to reach the clamped corner")
        path = output / "client-background-locate.ppm"
        qmp("screendump", {"filename": str(path), "format": "ppm"})
        blocks = solid_blocks(surface.ppm(path.read_bytes()))
        if len(blocks) != 1: raise RuntimeError("first actual preview is not uniquely visible")
        target = [blocks[0][0] + 16, blocks[0][1] + 16]
        for _ in range(24):
            if position == target:
                break
            previous = position
            position = motion(*[max(-100, min(100, target[i] - position[i])) for i in range(2)])
            if position == previous:
                raise RuntimeError("background pointer preparation made no actual progress")
        else:
            raise RuntimeError("background pointer preparation exceeded its bounded packet budget")
        for mask in (2, 4, 6, 0): button_packet(first, mask, False)
        focus(first)
        character(first, "a", "30d030")
        for mask in (0, 1, 3, 2, 6, 7, 5, 4, 0): button_packet(first, mask)
        second = launch(True)
        # The developer console deliberately revokes client routing. Observe
        # that loss before the new explicit gesture, so it cannot be mistaken
        # for a loss caused by the later unrelated publication.
        wait_regex(rb"NEXT APP pid=" + str(first["pid"]).encode() + rb": CLIENT INPUT EVENT: .*kind=2 ", second["start"])
        restored = refocus_here(first)
        before_publication = character(first, "a", "30d030")
        published = completed_launch(second)
        if published.start() < before_publication:
            raise RuntimeError("delayed client published before the first refocused character response; overlap proof not established")
        if re.search(rb"NEXT APP pid=" + str(first["pid"]).encode() + rb": CLIENT INPUT EVENT: .*kind=2 ", data[restored:published.end()]):
            raise RuntimeError("second publication stole focus from the refocused first client")
        character(first, "a", "30d030")
    else:
        second = launch()
        focus(first)
        character(first, "a", "30d030")
    focus(second)
    character(second, "b", "f0c020")
    pressure = len(data)
    key("p")
    wait_text(f"NEXT APP pid={second['pid']}: CLIENT INPUT PROBE: pausing dequeue for bounded overflow test", pressure)
    for _ in range(24):
        key("x")
        time.sleep(min(0.01, remaining()))
    reset = wait_regex(rb"NEXT APP pid=" + str(second["pid"]).encode() + rb": CLIENT INPUT EVENT: seq=(\d+) configure=\d+ kind=5 x=0 y=0 buttons=1 scalar=0 timestamp=(\d+)", pressure)
    lost = wait_regex(rb"NEXT APP pid=" + str(second["pid"]).encode() + rb": CLIENT INPUT EVENT: seq=(\d+) configure=\d+ kind=2 x=0 y=0 buttons=0 scalar=0 timestamp=(\d+)", reset.start())
    if int(lost[1]) != int(reset[1]) + 1:
        raise RuntimeError("overflow did not retain ordered reset and focus-loss events")
    focus(second)
    overlay = len(data)
    key("o")
    wait_text(f"NEXT APP pid={second['pid']}: CLIENT INPUT PROBE: requested trusted portal; secure overlay must suspend client input", overlay)
    wait_regex(rb"PRISM PORTAL: request=(\d+) pid=" + str(second["pid"]).encode() + rb" access=1\r?\n", overlay)
    key("f10")
    wait_text("PRISM FRAME:", overlay)
    guarded = len(data)
    key("a")
    key("f10")
    wait_text("PRISM FRAME:", guarded)
    if re.search(rb"NEXT APP pid=\d+: CLIENT INPUT EVENT: .*kind=3 .*scalar=97 ", data[guarded:]):
        raise RuntimeError("security-sensitive chooser forwarded a key to a client")
    capture("trusted-portal", second["bounds"])
    key("esc")
    focus(second)
    wait_text(f"NEXT APP pid={second['pid']}: CLIENT INPUT PROBE: trusted portal denied; client focus needs a new gesture", guarded)
    for client in (second, first):
        # Source13 intentionally suppresses reacquisition of the same focused
        # client. The second client already received the verified real gesture
        # after portal denial; send its exit key through that existing focus.
        if not THREE_BUTTON or client is not second:
            focus(client)
        start = len(data)
        key("q")
        wait_exit(client["pid"], 50, start)
        wait_text(f"PRISM SURFACE CLEARED: slot={client['slot']} generation={client['generation']} pid={client['pid']}", start)
        clients.remove(client)
    fixture_bindings = {fixture: manifest[fixture]}
    if THREE_BUTTON:
        fixture_bindings.update({name: manifest[name] for name in (
            "crates/next-user/src/client_input_fixture.rs",
            "crates/next-user/src/bin/client-input-delayed.rs")})
    report["client_input"] = {"fixture_bindings": fixture_bindings, "signed_package": PACKAGE, "exit_code": 50, "fixture_sha256": manifest[fixture], "validator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "three_button_packets": button_packets, "captures": captures, "overflow_reset_sequence": int(reset[1]), "overflow_focus_loss_sequence": int(lost[1]),
        "limits": "Trusted kernel-hosted Prism, semantic key press delivery only; no physical key-release or isolated compositor claim"}
    report["checks"].extend(["two signed clients receive input only after real pointer gestures and owned response colors appear in completed guest frames", "READ/WAIT separation and short/null/readonly/guard copyout preserve the actual head event", "actual bounded queue overflow retains ordered reset/focus loss and requires a fresh gesture", "trusted file chooser suspends client routing and fresh gesture restores it; both clients exit and exact generations clear"])
    if THREE_BUTTON:
        report["checks"].extend(["all eight actual mouse masks produce exactly one owned pointer event per packet including 3→2; repeated left/chords do not refocus the same client", "background right/middle never focus; delayed signed second publication preserves first-client focus after explicit refocus"])
    return original(serial, data, wait_text, wait_regex, wait_exit, qmp, remaining, report)


def main():
    global THREE_BUTTON
    if "--three-button-proof" in sys.argv:
        THREE_BUTTON = True
        sys.argv.remove("--three-button-proof")
    if "--self-test" in sys.argv:
        width, height = 600, 220
        rows = [bytearray(width * 3) for _ in range(height)]
        color = bytes.fromhex("3060b0")
        # The matching wallpaper stripe is too shallow; two actual-size solid
        # regions retain their locations even if a notification clips one.
        for y in range(8): rows[y][10 * 3:200 * 3] = color * 190
        for x, y, w, h in ((20, 20, 128, 96), (300, 120, 128, 70)):
            for row in range(y, y + h): rows[row][x * 3:(x + w) * 3] = color * w
        assert solid_blocks((width, height, b"".join(rows))) == [(20, 20), (300, 120)]
        captured = Path("build/eleventh-observations/client-pointer-motion-mismatch/client-locate.ppm")
        if captured.is_file(): assert solid_blocks(surface.ppm(captured.read_bytes())) == [(980, 166), (980, 460)]
        print("Actual solid-content locator host checks PASS; no guest boot performed")
        return 0
    surface.exercise_surface = exercise
    return surface.main()


if __name__ == "__main__":
    sys.exit(main())
