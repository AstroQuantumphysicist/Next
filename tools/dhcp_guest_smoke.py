#!/usr/bin/env python3
"""Exercise guest DHCP T1/T2/expiry against a deterministic Ethernet peer.

Unlike the host runtime harness, this boots the built kernel in QEMU and sends
real Ethernet frames through QEMU's socket network backend. The peer grants
short leases and controls ACK/NAK/drop behavior so the retained guest runtime
can be driven through renewal, rebinding, recovery and expiry in bounded time.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
CLIENT_IP = bytes((10, 0, 2, 15))
SERVER_IP = bytes((10, 0, 2, 2))
ALT_SERVER_IP = bytes((10, 0, 2, 3))
CLIENT_MAC = bytes.fromhex("525400123456")
SERVER_MAC = bytes.fromhex("52550a000202")
ALT_SERVER_MAC = bytes.fromhex("52550a000203")
NS = 1_000_000_000


def checksum(data):
    if len(data) & 1:
        data = bytes(data) + b"\0"
    total = sum((data[i] << 8) | data[i + 1] for i in range(0, len(data), 2))
    while total >> 16:
        total = (total & 0xFFFF) + (total >> 16)
    return (~total) & 0xFFFF


def ipv4_packet(source, destination, protocol, payload, ident=1):
    header = bytearray(20)
    header[0] = 0x45
    struct.pack_into("!HHHBBH4s4s", header, 2, len(header) + len(payload), ident,
                      0x4000, 64, protocol, 0, source, destination)
    struct.pack_into("!H", header, 10, checksum(header))
    return bytes(header) + payload


def ethernet(destination, source, kind, payload):
    return destination + source + struct.pack("!H", kind) + payload


def udp_frame(destination_mac, source_mac, source_ip, destination_ip,
              source_port, destination_port, payload):
    udp = struct.pack("!HHHH", source_port, destination_port, 8 + len(payload), 0) + payload
    return ethernet(destination_mac, source_mac, 0x0800,
                    ipv4_packet(source_ip, destination_ip, 17, udp))


def parse_options(data):
    options = {}
    position = 240
    while position < len(data):
        code = data[position]
        position += 1
        if code == 255:
            break
        if code == 0:
            continue
        if position >= len(data):
            return {}
        length = data[position]
        position += 1
        if position + length > len(data):
            return {}
        options[code] = data[position:position + length]
        position += length
    return options


def dhcp_message(frame):
    if len(frame) < 42 or frame[12:14] != b"\x08\x00":
        return None
    ip_start = 14
    ihl = (frame[ip_start] & 15) * 4
    if ihl < 20 or frame[ip_start + 9] != 17:
        return None
    udp_start = ip_start + ihl
    if len(frame) < udp_start + 8:
        return None
    source_port, destination_port, length, _ = struct.unpack_from("!HHHH", frame, udp_start)
    if (source_port, destination_port) != (68, 67) or length < 8 + 240:
        return None
    payload = frame[udp_start + 8:udp_start + length]
    if len(payload) < 240 or payload[236:240] != bytes((99, 130, 83, 99)):
        return None
    return {
        "xid": payload[4:8],
        "ciaddr": payload[12:16],
        "yiaddr": payload[16:20],
        "chaddr": payload[28:34],
        "options": parse_options(payload),
        "destination_mac": frame[:6],
        "destination_ip": frame[ip_start + 16:ip_start + 20],
    }


def make_dhcp_reply(kind, xid, server_ip, server_mac, lease=40, t1=12, t2=30,
                    ciaddr=bytes(4), yiaddr=CLIENT_IP, destination_ip=bytes((255, 255, 255, 255))):
    bootp = bytearray(240)
    bootp[0:3] = bytes((2, 1, 6))
    bootp[4:8] = xid
    bootp[12:16] = ciaddr
    bootp[16:20] = yiaddr
    bootp[20:24] = server_ip
    bootp[28:34] = CLIENT_MAC
    bootp[236:240] = bytes((99, 130, 83, 99))
    options = bytearray((53, 1, kind, 54, 4)) + bytearray(server_ip)
    if kind != 6:
        options.extend((1, 4, 255, 255, 255, 0, 3, 4, 10, 0, 2, 2,
                        6, 4, 10, 0, 2, 3, 51, 4))
        options.extend(struct.pack("!I", lease))
        options.extend((58, 4))
        options.extend(struct.pack("!I", t1))
        options.extend((59, 4))
        options.extend(struct.pack("!I", t2))
    options.append(255)
    payload = bytes(bootp + options)
    return udp_frame(CLIENT_MAC if destination_ip == CLIENT_IP else bytes.fromhex("ffffffffffff"),
                     server_mac, server_ip, destination_ip, 67, 68, payload)


def tcp_checksum(source, destination, segment):
    pseudo = source + destination + bytes((0, 6)) + struct.pack("!H", len(segment))
    return checksum(pseudo + segment)


def tcp_frame(destination_mac, source_mac, source_ip, destination_ip,
              source_port, destination_port, sequence, acknowledgment, flags, payload=b""):
    tcp = bytearray(20 + len(payload))
    struct.pack_into("!HHII", tcp, 0, source_port, destination_port, sequence, acknowledgment)
    tcp[12] = 5 << 4
    tcp[13] = flags
    struct.pack_into("!H", tcp, 14, 65535)
    tcp[20:] = payload
    struct.pack_into("!H", tcp, 16, tcp_checksum(source_ip, destination_ip, tcp))
    return ethernet(destination_mac, source_mac, 0x0800,
                    ipv4_packet(source_ip, destination_ip, 6, bytes(tcp)))


class EthernetPeer:
    """One QEMU TCP socket link and a small deterministic DHCP peer."""
    def __init__(self, listener):
        self.listener = listener
        self.connection = None
        self.stop = threading.Event()
        self.error = None
        self.started = time.monotonic()
        self.events = []
        self.dhcp_grants = 0
        self.dhcp_mode = "initial"
        self.lock = threading.Lock()
        self.thread = threading.Thread(target=self._serve, name="dhcp-guest-ethernet", daemon=True)

    def start(self):
        self.thread.start()

    def _record(self, name, **details):
        with self.lock:
            self.events.append({"event": name, "seconds": round(time.monotonic() - self.started, 3), **details})

    def snapshot(self):
        with self.lock:
            return list(self.events)

    def _send(self, frame):
        connection = self.connection
        if connection is None:
            return
        connection.sendall(struct.pack("!I", len(frame)) + frame)

    def _read_exact(self, size):
        result = bytearray()
        while len(result) < size and not self.stop.is_set():
            part = self.connection.recv(size - len(result))
            if not part:
                raise EOFError("QEMU socket network closed")
            result.extend(part)
        return bytes(result)

    def _serve(self):
        try:
            self.listener.settimeout(10)
            self.connection, _ = self.listener.accept()
            self.connection.settimeout(0.5)
            while not self.stop.is_set():
                try:
                    prefix = self._read_exact(4)
                except socket.timeout:
                    continue
                if not prefix:
                    break
                length = struct.unpack("!I", prefix)[0]
                if not 14 <= length <= 65536:
                    raise RuntimeError(f"invalid QEMU socket frame length {length}")
                self._handle(self._read_exact(length))
        except (OSError, EOFError) as error:
            if not self.stop.is_set():
                self.error = repr(error)
        except Exception as error:  # preserve peer failures for the supervising test
            self.error = repr(error)

    def _handle(self, frame):
        if len(frame) >= 42 and frame[12:14] == b"\x08\x06":
            if frame[20:22] == b"\x00\x01" and frame[38:42] == SERVER_IP:
                self._record("arp-request", target=frame[38:42].hex())
                guest_mac = frame[22:28]
                guest_ip = frame[28:32]
                reply = ethernet(guest_mac, SERVER_MAC, 0x0806,
                                 bytes((0, 1, 8, 0, 6, 4, 0, 2)) +
                                 SERVER_MAC + SERVER_IP + guest_mac + guest_ip)
                self._send(reply)
                self._record("arp-reply")
            return
        dhcp = dhcp_message(frame)
        if dhcp is not None:
            self._handle_dhcp(dhcp)
            return
        if len(frame) < 34 or frame[12:14] != b"\x08\x00":
            return
        ihl = (frame[14] & 15) * 4
        if ihl < 20 or len(frame) < 14 + ihl:
            return
        source_ip = frame[26:30]
        destination_ip = frame[30:34]
        protocol = frame[23]
        if protocol == 1:
            self._handle_icmp(frame, ihl, source_ip, destination_ip)
        elif protocol == 17:
            self._handle_udp(frame, ihl, source_ip, destination_ip)
        elif protocol == 6:
            self._handle_tcp(frame, ihl, source_ip, destination_ip)

    def _handle_dhcp(self, message):
        options = message["options"]
        kind = options.get(53, b"\0")[0]
        xid = message["xid"]
        ciaddr = message["ciaddr"]
        is_rebind = message["destination_mac"] == bytes.fromhex("ffffffffffff")
        if kind == 1:
            if self.dhcp_grants >= 4:
                self.dhcp_mode = "expired"
                self._record("lease-expiry-discovery", xid=xid.hex())
                return
            self._record("discover", grant=self.dhcp_grants, xid=xid.hex())
            self._send(make_dhcp_reply(2, xid, SERVER_IP, SERVER_MAC, ciaddr=bytes(4), yiaddr=CLIENT_IP))
            return
        if kind != 3:
            return
        # An initial/recovery SELECT request carries option 50; renewal and
        # rebind requests carry ciaddr and omit both 50 and 54.
        if ciaddr == bytes(4):
            self._record("select", grant=self.dhcp_grants, xid=xid.hex())
            server = options.get(54, SERVER_IP)
            reply_ip = server if len(server) == 4 else SERVER_IP
            reply_mac = ALT_SERVER_MAC if reply_ip == ALT_SERVER_IP else SERVER_MAC
            self._send(make_dhcp_reply(5, xid, reply_ip, reply_mac, ciaddr=bytes(4), yiaddr=CLIENT_IP,
                                       lease=40, t1=12, t2=30))
            self.dhcp_grants += 1
            self.dhcp_mode = "bound"
            self._record("reacquire-ack" if self.dhcp_grants == 4 else "initial-ack",
                         grant=self.dhcp_grants)
            return
        if self.dhcp_grants == 1:
            self._record("t1-renewal", xid=xid.hex(), broadcast=False)
            self._send(make_dhcp_reply(5, xid, SERVER_IP, SERVER_MAC, ciaddr=CLIENT_IP,
                                       yiaddr=bytes(4), lease=40, t1=12, t2=30,
                                       destination_ip=CLIENT_IP))
            self.dhcp_grants += 1
            self._record("t1-ack-refresh", grant=self.dhcp_grants)
            return
        if self.dhcp_grants == 2 and is_rebind:
            self._record("t2-rebind", xid=xid.hex(), broadcast=True)
            self._send(make_dhcp_reply(5, xid, ALT_SERVER_IP, ALT_SERVER_MAC, ciaddr=CLIENT_IP,
                                       yiaddr=bytes(4), lease=40, t1=12, t2=30,
                                       destination_ip=bytes((255, 255, 255, 255))))
            self.dhcp_grants += 1
            self._record("t2-ack-refresh", grant=self.dhcp_grants, server=ALT_SERVER_IP.hex())
            return
        if self.dhcp_grants == 2:
            self._record("t1-renewal-dropped", xid=xid.hex(), broadcast=False)
            return
        if self.dhcp_grants == 3 and not is_rebind:
            server = options.get(54, ALT_SERVER_IP)
            if len(server) != 4:
                server = ALT_SERVER_IP
            self._record("t1-renewal-nak", xid=xid.hex(), server=server.hex())
            self._send(make_dhcp_reply(6, xid, server, ALT_SERVER_MAC,
                                       ciaddr=bytes(4), yiaddr=bytes(4),
                                       destination_ip=bytes((255, 255, 255, 255))))
            self.dhcp_mode = "nak-seen"
            return
        if self.dhcp_grants == 4:
            self._record("final-lease-renewal-dropped", xid=xid.hex(), broadcast=is_rebind)
            return
        self._record("unexpected-dhcp-request", message_type=kind, grants=self.dhcp_grants,
                     broadcast=is_rebind)

    def _handle_icmp(self, frame, ihl, source_ip, destination_ip):
        start = 14 + ihl
        total = struct.unpack_from("!H", frame, 16)[0]
        packet = bytearray(frame[start:14 + total])
        if not packet or packet[0] != 8:
            return
        self._record("icmp-request", length=len(packet), source=source_ip.hex(),
                     destination=destination_ip.hex(), mac=frame[6:12].hex(),
                     icmp=packet.hex())
        packet[0] = 0
        packet[2:4] = b"\0\0"
        packet[2:4] = struct.pack("!H", checksum(packet))
        reply = ethernet(frame[6:12], SERVER_MAC, 0x0800,
                         ipv4_packet(destination_ip, source_ip, 1, bytes(packet)))
        self._send(reply)
        self._record("icmp-reply", ip_checksum=struct.unpack_from("!H", reply, 24)[0],
                     icmp=packet.hex())

    def _handle_udp(self, frame, ihl, source_ip, destination_ip):
        start = 14 + ihl
        if len(frame) < start + 8:
            return
        source_port, destination_port, length, _ = struct.unpack_from("!HHHH", frame, start)
        if length < 8 or start + length > len(frame):
            return
        payload = frame[start + 8:start + length]
        if destination_port == 39099:
            response = dns_reply(payload)
            if response is not None:
                self._send(udp_frame(frame[6:12], SERVER_MAC, SERVER_IP, source_ip,
                                     39099, source_port, response))
        elif destination_port == 39097:
            self._send(udp_frame(frame[6:12], SERVER_MAC, SERVER_IP, source_ip,
                                 39097, source_port, payload))

    def _handle_tcp(self, frame, ihl, source_ip, destination_ip):
        start = 14 + ihl
        if len(frame) < start + 20:
            return
        source_port, destination_port, sequence, acknowledgment = struct.unpack_from("!HHII", frame, start)
        offset = (frame[start + 12] >> 4) * 4
        flags = frame[start + 13]
        payload = frame[start + offset:14 + struct.unpack_from("!H", frame, 16)[0]]
        response = (b"HTTP/1.0 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\n"
                    b"NEXTTCP1pong")
        if destination_port != 39098:
            return
        if flags & 0x02:
            reply = tcp_frame(frame[6:12], SERVER_MAC, SERVER_IP, source_ip,
                              39098, source_port, 0x12345678, sequence + 1, 0x12)
            self._send(reply)
        elif payload:
            self._send(tcp_frame(frame[6:12], SERVER_MAC, SERVER_IP, source_ip,
                                 39098, source_port, 0x12345679, sequence + len(payload),
                                 0x18, response))
        elif flags & 0x01:
            self._send(tcp_frame(frame[6:12], SERVER_MAC, SERVER_IP, source_ip,
                                 39098, source_port, 0x12345679 + len(response), sequence + 1,
                                 0x11))
        elif flags & 0x10:
            return

    def close(self):
        self.stop.set()
        if self.connection is not None:
            try:
                self.connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            self.connection.close()
        self.listener.close()
        self.thread.join(timeout=2)


def dns_reply(packet):
    if len(packet) < 17:
        return None
    ident = packet[:2]
    position = 12
    while position < len(packet) and packet[position] != 0:
        length = packet[position]
        if length > 63 or position + 1 + length > len(packet):
            return None
        position += 1 + length
    if position + 5 > len(packet):
        return None
    question = packet[12:position + 5]
    query_type = struct.unpack_from("!H", packet, position + 1)[0]
    if query_type == 1:
        address = bytes((10, 0, 2, 2))
    elif query_type == 28:
        address = bytes(15) + b"\x01"
    else:
        return None
    answer = b"\xc0\x0c" + struct.pack("!HHIH", query_type, 1, 2, len(address)) + address
    return ident + struct.pack("!HHHHH", 0x8180, 1, 1, 0, 0) + question + answer


def read_serial(process, buffer, deadline):
    while time.monotonic() < deadline:
        ready, _, _ = select.select([process.stdout], [], [], min(0.25, deadline - time.monotonic()))
        if not ready:
            if process.poll() is not None:
                raise RuntimeError(f"QEMU exited with status {process.returncode}")
            continue
        chunk = os.read(process.stdout.fileno(), 4096)
        if not chunk:
            detail = buffer.decode("utf-8", errors="replace")[-2000:]
            raise RuntimeError(f"QEMU serial stream closed (status {process.poll()}): {detail}")
        buffer.extend(chunk)
        yield chunk


def validate_timing(events):
    def find(name, predicate=lambda event: True):
        try:
            return next(event for event in events
                        if event["event"] == name and predicate(event))
        except StopIteration as error:
            raise RuntimeError(f"missing transition {name}") from error

    initial = find("initial-ack", lambda event: event.get("grant") == 1)
    first_t1 = find("t1-renewal")
    refreshed = find("t1-ack-refresh")
    second_t1 = find("t1-renewal-dropped")
    rebind = find("t2-rebind")
    rebind_ack = find("t2-ack-refresh")
    nak = find("t1-renewal-nak")
    recovery_discover = find("discover", lambda event: event.get("grant") == 3)
    recovery_ack = find("reacquire-ack")
    expiry = find("lease-expiry-discovery")
    intervals = {
        "initial_ack_to_t1_seconds": first_t1["seconds"] - initial["seconds"],
        "refreshed_ack_to_next_t1_seconds": second_t1["seconds"] - refreshed["seconds"],
        "refreshed_ack_to_t2_seconds": rebind["seconds"] - refreshed["seconds"],
        "rebind_ack_to_nak_seconds": nak["seconds"] - rebind_ack["seconds"],
        "nak_to_restart_discover_seconds": recovery_discover["seconds"] - nak["seconds"],
        "reacquire_ack_to_expiry_discover_seconds": expiry["seconds"] - recovery_ack["seconds"],
    }
    bounds = {
        "initial_ack_to_t1_seconds": (10.0, 15.0),
        "refreshed_ack_to_next_t1_seconds": (10.0, 15.0),
        "refreshed_ack_to_t2_seconds": (27.0, 34.0),
        "rebind_ack_to_nak_seconds": (10.0, 15.0),
        "nak_to_restart_discover_seconds": (0.0, 2.0),
        "reacquire_ack_to_expiry_discover_seconds": (38.0, 45.0),
    }
    for name, (minimum, maximum) in bounds.items():
        if not minimum <= intervals[name] <= maximum:
            raise RuntimeError(f"DHCP timer {name}={intervals[name]:.3f}s outside {minimum}..{maximum}s")
    ordered = [initial, first_t1, refreshed, second_t1, rebind, rebind_ack, nak,
               recovery_discover, recovery_ack, expiry]
    if [events.index(event) for event in ordered] != sorted(events.index(event) for event in ordered):
        raise RuntimeError("guest DHCP transition order is inconsistent")
    if rebind.get("broadcast") is not True or first_t1.get("broadcast") is not False:
        raise RuntimeError("guest did not use unicast T1 followed by broadcast T2")
    if rebind_ack.get("server") != ALT_SERVER_IP.hex():
        raise RuntimeError("T2 ACK did not come from the alternate server")
    return {name: round(value, 3) for name, value in intervals.items()}


def run(args):
    image = args.image.resolve()
    if not image.is_file():
        raise RuntimeError(f"missing boot image: {image}; run tools/build.sh")
    sysroot = Path(os.environ.get("NEXT_SYSROOT", "/workspace/toolchains/sysroot"))
    local_qemu = sysroot / "usr/bin/qemu-system-x86_64"
    qemu = str(local_qemu) if local_qemu.exists() else shutil.which("qemu-system-x86_64")
    if not qemu:
        raise RuntimeError("qemu-system-x86_64 is missing; source tools/env.sh")
    firmware = sysroot / "usr/share/OVMF"
    if not firmware.exists():
        firmware = Path("/usr/share/OVMF")
    if not (firmware / "OVMF_CODE_4M.fd").exists():
        raise RuntimeError(f"OVMF firmware missing below {firmware}")
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    peer = EthernetPeer(listener)
    peer.start()
    source_paths = [ROOT / "crates/helix-kernel/src/network.rs",
                    ROOT / "crates/helix-kernel/src/network_runtime.rs",
                    ROOT / "crates/helix-kernel/src/network_dhcp.rs",
                    ROOT / "tools/dhcp_guest_smoke.py"]
    source_hashes = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                     for path in source_paths}
    report = {
        "passed": False,
        "scope": "QEMU q35/TCG guest kernel through virtio-net and a deterministic Ethernet peer; no hardware certification",
        "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "image_sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
        "source_sha256": source_hashes,
        "qemu_profile": f"q35/TCG; cpu={args.cpu}; 256 MiB; one vCPU",
        "link": "QEMU socket netdev TCP Ethernet frames",
        "lease_policy": {"initial_seconds": 40, "t1_seconds": 12, "t2_seconds": 30,
                         "rebind_ack_server": "10.0.2.3", "post_nak_lease_seconds": 40},
    }
    process = None
    serial = bytearray()
    qemu_output = bytearray()
    try:
        with tempfile.TemporaryDirectory(prefix="next-dhcp-guest-") as temp_name:
            temp = Path(temp_name)
            variables = temp / "vars.fd"
            shutil.copyfile(firmware / "OVMF_VARS_4M.fd", variables)
            environment = os.environ.copy()
            if local_qemu.exists():
                lib = str(sysroot / "usr/lib/x86_64-linux-gnu")
                environment["LD_LIBRARY_PATH"] = lib + (":" + environment["LD_LIBRARY_PATH"] if environment.get("LD_LIBRARY_PATH") else "")
            net_id = listener.getsockname()[1]
            command = [qemu, "-machine", "q35,accel=tcg", "-cpu", args.cpu, "-m", "256M", "-smp", "1", "-vga", "std",
                       "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE_4M.fd'}",
                       "-drive", f"if=pflash,format=raw,file={variables}",
                       "-drive", f"if=none,id=nextdisk,format=raw,snapshot=on,file={image}",
                       "-device", "virtio-blk-pci,drive=nextdisk,disable-modern=on",
                       "-netdev", f"socket,id=nextnet,connect=127.0.0.1:{net_id}",
                       "-device", "virtio-net-pci,netdev=nextnet,disable-modern=on",
                       "-audiodev", f"wav,id=nextaudio,path={temp / 'audio.wav'},out.frequency=48000,out.channels=2,out.format=s16",
                       "-device", "AC97,audiodev=nextaudio", "-display", "none", "-monitor", "none",
                       "-serial", "stdio", "-no-reboot"]
            for data in ("usr/share/qemu", "usr/share/seabios", "usr/lib/ipxe/qemu"):
                if (sysroot / data).exists():
                    command.extend(("-L", str(sysroot / data)))
            process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                       stdin=subprocess.DEVNULL, env=environment, bufsize=0)
            deadline = time.monotonic() + args.timeout
            ready = False
            for chunk in read_serial(process, serial, deadline):
                qemu_output.extend(chunk)
                if b"PRISM: READY" in serial:
                    ready = True
                    break
            if not ready:
                raise TimeoutError("guest did not reach PRISM: READY")
            report["guest_ready_seconds"] = round(time.monotonic() - peer.started, 3)
            required = ["t1-renewal", "t1-ack-refresh", "t1-renewal-dropped", "t2-rebind",
                        "t2-ack-refresh", "t1-renewal-nak", "reacquire-ack", "lease-expiry-discovery"]
            while time.monotonic() < deadline:
                if peer.error:
                    raise RuntimeError(f"Ethernet peer failed: {peer.error}")
                events = peer.snapshot()
                kinds = [event["event"] for event in events]
                if all(expected in kinds for expected in required):
                    break
                if process.poll() is not None:
                    raise RuntimeError(f"QEMU exited with status {process.returncode}")
                for chunk in read_serial(process, serial, min(deadline, time.monotonic() + 0.25)):
                    qemu_output.extend(chunk)
            events = peer.snapshot()
            kinds = [event["event"] for event in events]
            missing = [event for event in required if event not in kinds]
            if missing:
                raise TimeoutError(f"missing guest DHCP transitions: {missing}; events={kinds}")
            report["timer_intervals_seconds"] = validate_timing(events)
            final_source_hashes = {
                str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in source_paths
            }
            if final_source_hashes != source_hashes:
                raise RuntimeError("network sources changed during the QEMU guest run")
            if b"HELIX DHCP PASS" not in serial or b"PRISM: READY" not in serial:
                raise RuntimeError("guest never logged DHCP acquisition and desktop readiness")
            report["events"] = events
            report["checks"] = [
                "QEMU guest acquired the peer's short DHCP lease before desktop startup",
                "retained guest runtime emitted a T1 unicast renewal and accepted an ACK that refreshed its lease",
                "a later T1 was dropped; the guest emitted a T2 broadcast rebind and accepted an ACK from another server",
                "the guest emitted another T1 request, received a DHCP NAK, and restarted discovery",
                "after reacquisition, renewal and rebind were dropped; lease expiry restarted discovery",
            ]
            report["serial_tail"] = serial.decode("utf-8", errors="replace")[-5000:]
            report["passed"] = True
    except Exception as error:
        report["error"] = str(error)
        report["events"] = peer.snapshot()
        report["peer_error"] = peer.error
        report["serial_tail"] = serial.decode("utf-8", errors="replace")[-12000:]
        report_path = args.output.resolve()
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_text(json.dumps(report, indent=2) + "\n")
        raise
    finally:
        peer.close()
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
    report["events"] = peer.snapshot()
    report_path = args.output.resolve()
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "report": str(report_path),
                      "transitions": [event["event"] for event in report["events"]]}, indent=2))
    return 0 if report["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cpu", choices=("qemu64", "max"), default="qemu64")
    parser.add_argument("--timeout", type=float, default=150.0)
    parser.add_argument("--image", type=Path, default=ROOT / "build/next-dev.img")
    parser.add_argument("--output", type=Path, default=ROOT / "build/dhcp-guest-smoke.json")
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("--timeout must be at least one second")
    try:
        return run(args)
    except Exception as error:
        print(f"DHCP guest smoke failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
