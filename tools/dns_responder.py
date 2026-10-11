#!/usr/bin/env python3
"""Loopback-only original DNS peer for QEMU UDP resolver tests.

Answers next.test A=10.0.2.2 and AAAA=::1, TTL=2 seconds. Unknown names
receive authoritative NXDOMAIN with SOA negative TTL=2. No upstream traffic.
"""
import argparse
import json
import signal
import socket
import struct
import threading

MAX_PACKET = 512
MAX_FLOWS = 128


def reply(packet):
    """Return one bounded response, or None for malformed/unsupported framing."""
    if not 12 <= len(packet) <= MAX_PACKET:
        return None
    ident, flags, questions, answers, authority, additional = struct.unpack('!6H', packet[:12])
    if flags & 0xF840 or questions != 1 or answers or authority or additional:
        return None
    position = 12
    labels = []
    name_bytes = 0
    while position < len(packet):
        length = packet[position]
        position += 1
        if length == 0:
            break
        if length > 63 or position + length > len(packet):
            return None
        label = packet[position:position + length]
        if not all(65 <= byte <= 90 or 97 <= byte <= 122 or 48 <= byte <= 57 or byte == 45 for byte in label):
            return None
        labels.append(label.lower())
        name_bytes += length + (1 if len(labels) > 1 else 0)
        if name_bytes > 253:
            return None
        position += length
    else:
        return None
    if not labels or position + 4 != len(packet):
        return None
    kind, family = struct.unpack('!HH', packet[position:position + 4])
    if family != 1:
        return None
    question = packet[12:]
    hostname = b'.'.join(labels)
    known = hostname == b'next.test'
    rd = flags & 0x100
    # Reserved loopback fixture names exercise explicit terminal outcomes.
    # No upstream or arbitrary per-query destination is ever consulted.
    if hostname == b'dns-timeout.next.test':
        return b''
    if hostname == b'dns-malformed.next.test':
        return struct.pack('!6H', ident ^ 1, 0x8480 | rd, 1, 1, 0, 0) + question + b'\xc0'
    if hostname == b'dns-truncated.next.test':
        return struct.pack('!6H', ident, 0x8680 | rd, 1, 0, 0, 0) + question
    if hostname in (b'dns-alias.next.test', b'dns-referral.next.test'):
        target = b'\x04next\x04test\x00'
        alias = hostname == b'dns-alias.next.test'
        header = struct.pack('!6H', ident, 0x8480 | rd, 1, int(alias), int(not alias), 0)
        record = b'\xc0\x0c' + struct.pack('!HHIH', 5 if alias else 2, 1, 2, len(target)) + target
        return header + question + record
    if known and kind in (1, 28):
        address = bytes((10, 0, 2, 2)) if kind == 1 else bytes(15) + b'\x01'
        header = struct.pack('!6H', ident, 0x8480 | rd, 1, 1, 0, 0)
        record = b'\xc0\x0c' + struct.pack('!HHIH', kind, 1, 2, len(address)) + address
    else:
        header = struct.pack('!6H', ident, 0x8480 | rd | (0 if known else 3), 1, 0, 1, 0)
        soa = b'\xc0\x0c\xc0\x0c' + struct.pack('!5I', 1, 60, 30, 300, 2)
        record = b'\xc0\x0c' + struct.pack('!HHIH', 6, 1, 2, len(soa)) + soa
    return header + question + record


class Responder:
    """One daemon-owned socket, bounded datagrams, and integer-only statistics."""
    def __init__(self, port=39099, drop_first=False):
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        # Linux hashes each UDP source flow to one reuseport socket. Peer
        # counters are per socket, not attributable to the launching VM.
        if hasattr(socket, 'SO_REUSEPORT'):
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        self.socket.bind(('127.0.0.1', port))
        self.port = self.socket.getsockname()[1]
        self.socket.settimeout(0.1)
        self.stopped = threading.Event()
        self.stats = {'requests': 0, 'answers': 0, 'malformed': 0, 'dropped': 0,
                      'boot_requests': 0, 'boot_answers': 0, 'boot_dropped': 0}
        self.boot_flow = None
        self.drop_first = drop_first
        self.dropped_flows = set()
        self.thread = threading.Thread(target=self._serve, name='next-dns-peer', daemon=True)
        self.thread.start()

    def _serve(self):
        while not self.stopped.is_set():
            try:
                packet, source = self.socket.recvfrom(MAX_PACKET + 1)
            except socket.timeout:
                continue
            except OSError:
                break
            self.stats['requests'] += 1
            response = reply(packet)
            if response is None:
                self.stats['malformed'] += 1
                continue
            if self.boot_flow is None:
                self.boot_flow = source
            boot_flow = source == self.boot_flow
            if boot_flow:
                self.stats['boot_requests'] += 1
            if response == b'':
                self.stats['dropped'] += 1
                if boot_flow:
                    self.stats['boot_dropped'] += 1
                continue
            if self.drop_first and source not in self.dropped_flows:
                if len(self.dropped_flows) == MAX_FLOWS:
                    self.stats['malformed'] += 1
                    continue
                self.dropped_flows.add(source)
                self.stats['dropped'] += 1
                if boot_flow:
                    self.stats['boot_dropped'] += 1
                continue
            try:
                self.socket.sendto(response, source)
                self.stats['answers'] += 1
                if boot_flow:
                    self.stats['boot_answers'] += 1
            except OSError:
                if not self.stopped.is_set():
                    self.stats['malformed'] += 1

    def close(self):
        self.stopped.set()
        self.socket.close()
        self.thread.join(timeout=1)
        if self.thread.is_alive():
            raise RuntimeError('DNS peer failed to stop within bounded deadline')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=39099)
    parser.add_argument('--drop-first', action='store_true', help='exercise client timed retry')
    args = parser.parse_args()
    if not 0 <= args.port <= 65535:
        parser.error('port must be between 0 and 65535')
    peer = Responder(args.port, drop_first=args.drop_first)
    stopping = threading.Event()
    def stop(_signum, _frame):
        stopping.set()
    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    print(json.dumps({'host': '127.0.0.1', 'port': peer.port, 'zone': 'next.test', 'upstream': False}), flush=True)
    try:
        stopping.wait()
    finally:
        peer.close()
        print(json.dumps(peer.stats), flush=True)


if __name__ == '__main__':
    main()
