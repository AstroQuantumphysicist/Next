"""Deterministic deadline, quota and flow-isolation checks for the UDP peer."""
import unittest
from udp_delay import Scheduled, DELAY_NS, MAX_PENDING, MAX_BATCH


def request(mode, nonce=1):
    return b'NEXTWAIT' + mode.to_bytes(8, 'little') + nonce.to_bytes(8, 'little')


class ScheduledTests(unittest.TestCase):
    def test_delay_omit_and_flow_independence(self):
        peer = Scheduled()
        first = ('127.0.0.1', 50001)
        second = ('127.0.0.1', 50002)
        self.assertTrue(peer.accept(request(1, 1), first, 100))
        self.assertTrue(peer.accept(request(2, 2), second, 101))
        self.assertTrue(peer.accept(request(1, 3), second, 102))
        self.assertEqual(peer.due(100 + DELAY_NS - 1), [])
        self.assertEqual(peer.due(100 + DELAY_NS), [(first, request(1, 1))])
        self.assertEqual(peer.due(102 + DELAY_NS), [(second, request(1, 3))])
        self.assertEqual(peer.flows, {})
        self.assertEqual(peer.stats['omitted'], 1)

    def test_global_and_per_flow_quotas_and_bounded_release(self):
        peer = Scheduled()
        source = ('127.0.0.1', 50001)
        for nonce in range(5):
            self.assertTrue(peer.accept(request(1, nonce), source, 0))
        self.assertEqual(len(peer.pending), 4)
        self.assertEqual(peer.stats['overflow'], 1)
        for nonce in range(MAX_PENDING):
            peer.accept(request(1, nonce), ('127.0.0.1', 51000 + nonce), 0)
        self.assertEqual(len(peer.pending), MAX_PENDING)
        self.assertEqual(len(peer.due(DELAY_NS)), MAX_BATCH)
        self.assertEqual(len(peer.pending), MAX_PENDING - MAX_BATCH)
        self.assertGreater(peer.stats['overflow'], 1)

    def test_strict_synthetic_request_and_monotonic_clock(self):
        peer = Scheduled()
        source = ('127.0.0.1', 50001)
        for packet in [request(0), request(3), request(1) + b'x', b'NEXTWAIT', b'NEXTNONC' + bytes(16)]:
            self.assertFalse(peer.accept(packet, source, 100))
        self.assertFalse(peer.accept(request(1), ('192.0.2.1', 50000), 100))
        self.assertEqual(peer.timeout(100), 0.1)
        peer.accept(request(1), source, 100)
        self.assertEqual(peer.timeout(100 + DELAY_NS), 0)
        with self.assertRaises(ValueError):
            peer.due(0)

    def test_released_storage_is_reusable_and_snapshots_packet_bytes(self):
        peer = Scheduled()
        source = ('127.0.0.1', 50001)
        mutable = bytearray(request(1, 99))
        self.assertTrue(peer.accept(mutable, source, 0))
        mutable[-1] ^= 0xff
        self.assertEqual(peer.due(DELAY_NS), [(source, request(1, 99))])
        for cycle in range(1, 65):
            start = cycle * DELAY_NS
            for nonce in range(4):
                peer.accept(request(1, nonce), source, start)
            self.assertEqual(len(peer.pending), 4)
            self.assertEqual(len(peer.due(start + DELAY_NS)), 4)
            self.assertEqual(peer.pending, [])
            self.assertEqual(peer.flows, {})
        self.assertEqual(peer.stats['overflow'], 0)

    def test_equal_deadline_batches_preserve_fifo_across_flows(self):
        peer = Scheduled()
        expected = []
        for nonce in range(MAX_PENDING):
            source = ('127.0.0.1', 51000 + nonce)
            packet = request(1, nonce)
            peer.accept(packet, source, 0)
            expected.append((source, packet))
        released = []
        while peer.pending:
            batch = peer.due(DELAY_NS)
            self.assertLessEqual(len(batch), MAX_BATCH)
            released.extend(batch)
        self.assertEqual(released, expected)
        self.assertEqual(peer.flows, {})
        self.assertEqual(peer.stats['released'], MAX_PENDING)


if __name__ == '__main__':
    unittest.main()
