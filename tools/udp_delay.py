"""Bounded synthetic wait-fixture responses for the loopback development peer.

24 bytes: NEXTWAIT + little-endian mode64 + nonce64. Mode1 delays its exact
reply200ms; mode2 deliberately omits a reply. No request chooses an address,
port, delay, executable command or upstream service.
"""
import heapq

MAX_PENDING = 64
MAX_PER_FLOW = 4
MAX_BATCH = 16
DELAY_NS = 200_000_000


class Scheduled:
    def __init__(self):
        self.pending = []
        self.flows = {}
        self.sequence = 0
        self.last_time = 0
        self.stats = {'scheduled': 0, 'omitted': 0, 'overflow': 0, 'released': 0}

    def _advance(self, now):
        if now < self.last_time:
            raise ValueError('monotonic clock regressed')
        self.last_time = now

    def accept(self, packet, source, now):
        """Recognize the fixed synthetic request; never block a receiving flow."""
        self._advance(now)
        if (len(packet) != 24 or packet[:8] != b'NEXTWAIT'
                or source[0] != '127.0.0.1' or not 0 < source[1] <= 65535):
            return False
        mode = int.from_bytes(packet[8:16], 'little')
        if mode == 2:
            self.stats['omitted'] += 1
            return True
        if mode != 1:
            return False
        count = self.flows.get(source, 0)
        if len(self.pending) >= MAX_PENDING or count >= MAX_PER_FLOW:
            self.stats['overflow'] += 1
            return True
        self.sequence += 1
        heapq.heappush(self.pending, (now + DELAY_NS, self.sequence, source, bytes(packet)))
        self.flows[source] = count + 1
        self.stats['scheduled'] += 1
        return True

    def due(self, now):
        """Release at most16 due replies. Output peers came from recvfrom only."""
        self._advance(now)
        ready = []
        while self.pending and self.pending[0][0] <= now and len(ready) < MAX_BATCH:
            _, _, source, packet = heapq.heappop(self.pending)
            count = self.flows[source] - 1
            if count:
                self.flows[source] = count
            else:
                del self.flows[source]
            ready.append((source, packet))
            self.stats['released'] += 1
        return ready

    def timeout(self, now):
        """Wait at most100ms, shortened for a pending deadline."""
        self._advance(now)
        if not self.pending:
            return 0.1
        return max(0, min(0.1, (self.pending[0][0] - now) / 1_000_000_000))
