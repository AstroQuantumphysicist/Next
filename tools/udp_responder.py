"""Bounded loopback-only peer for the original Next UDP transport test."""
import socket
import threading
import time
from udp_delay import Scheduled


class Responder:
    def __init__(self, port=39097):
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        # Concurrent independent QEMU validation jobs share the same test peer.
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        self.socket.bind(("127.0.0.1", port))
        self.socket.settimeout(0.1)
        self.stop = threading.Event()
        self.requests = 0
        self.scheduled = Scheduled()
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        while not self.stop.is_set() and (self.requests < 1024 or self.scheduled.pending):
            try:
                for source, packet in self.scheduled.due(time.monotonic_ns()):
                    self.socket.sendto(packet, source)
                self.socket.settimeout(max(0.001, self.scheduled.timeout(time.monotonic_ns())))
                data, peer = self.socket.recvfrom(1473)
                if self.requests >= 1024:
                    continue
                if peer[0] == "127.0.0.1" and data == b"NEXTUDP1ping":
                    self.socket.sendto(b"NEXTUDP1pong", peer)
                    self.requests += 1
                elif peer[0] == "127.0.0.1" and (not data or (len(data) == 24 and data[:8] == b"NEXTNONC")):
                    self.socket.sendto(data, peer)
                    self.requests += 1
                elif self.scheduled.accept(data, peer, time.monotonic_ns()):
                    self.requests += 1
            except socket.timeout:
                continue
            except OSError:
                return

    def close(self):
        self.stop.set()
        self.socket.close()
        self.thread.join(timeout=1)


if __name__ == "__main__":
    import signal
    responder = Responder()
    try:
        signal.pause()
    finally:
        responder.close()
