"""Real loopback regression for the bounded development UDP fixture peer."""
import socket
import time
import unittest
from udp_responder import Responder


class UdpResponderTests(unittest.TestCase):
    def test_original_nonce_empty_echo_and_unrecognized_payload_rejection(self):
        responder = Responder(0)
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                client.settimeout(1)
                peer = responder.socket.getsockname()
                nonce = b"NEXTNONC" + (42).to_bytes(8, "little") + (1234).to_bytes(8, "little")
                for request, expected in [(b"NEXTUDP1ping", b"NEXTUDP1pong"), (nonce, nonce), (b"", b"")]:
                    client.sendto(request, peer)
                    reply, source = client.recvfrom(1473)
                    self.assertEqual(reply, expected)
                    self.assertEqual(source, peer)
                for malformed in [b"NEXTNONC", nonce + b"x", b"NEXTNONX" + nonce[8:]]:
                    client.sendto(malformed, peer)
                    client.settimeout(0.1)
                    with self.assertRaises(socket.timeout):
                        client.recvfrom(1473)
        finally:
            responder.close()

    def test_delayed_omitted_and_immediate_flows_are_independent(self):
        responder = Responder(0)
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as slow, \
                    socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as immediate:
                peer = responder.socket.getsockname()
                delayed = b"NEXTWAIT" + (1).to_bytes(8, "little") + (42).to_bytes(8, "little")
                omitted = b"NEXTWAIT" + (2).to_bytes(8, "little") + (43).to_bytes(8, "little")
                start = time.monotonic()
                slow.sendto(delayed, peer)
                immediate.settimeout(0.15)
                immediate.sendto(b"NEXTUDP1ping", peer)
                self.assertEqual(immediate.recvfrom(1473)[0], b"NEXTUDP1pong")
                slow.settimeout(1)
                response, source = slow.recvfrom(1473)
                self.assertEqual(response, delayed)
                self.assertEqual(source, peer)
                self.assertGreaterEqual(time.monotonic() - start, 0.18)
                slow.sendto(omitted, peer)
                slow.settimeout(0.25)
                with self.assertRaises(socket.timeout):
                    slow.recvfrom(1473)
                immediate.sendto(b"", peer)
                self.assertEqual(immediate.recvfrom(1473)[0], b"")
        finally:
            responder.close()
        self.assertEqual(responder.scheduled.stats['scheduled'], 1)
        self.assertEqual(responder.scheduled.stats['omitted'], 1)
        self.assertEqual(responder.scheduled.stats['released'], 1)


if __name__ == "__main__":
    unittest.main()
