#!/usr/bin/env python3
"""Deterministic original DNS peer checks, including actual loopback UDP."""
import socket
import struct
import unittest
from dns_responder import Responder, reply


def query(host=b'next.test', kind=1):
    name = b''.join(bytes((len(label),)) + label for label in host.split(b'.')) + b'\0'
    return struct.pack('!6H', 0x1234, 0x100, 1, 0, 0, 0) + name + struct.pack('!HH', kind, 1)


class DnsPeerTests(unittest.TestCase):
    def test_a_aaaa_and_negative_soa(self):
        for kind, address in [(1, bytes((10, 0, 2, 2))), (28, bytes(15) + b'\x01')]:
            packet = query(kind=kind)
            response = reply(packet)
            self.assertEqual(response[:2], packet[:2])
            self.assertEqual(struct.unpack('!H', response[6:8])[0], 1)
            self.assertEqual(response[-len(address):], address)
        response = reply(query(b'unknown.test'))
        self.assertEqual(response[3] & 15, 3)
        self.assertEqual(response[-4:], struct.pack('!I', 2))
        response = reply(query(kind=16))
        self.assertEqual(response[3] & 15, 0)
        self.assertEqual(struct.unpack('!H', response[8:10])[0], 1)

    def test_reserved_fault_outcomes_preserve_transaction_and_question(self):
        for host in (b'dns-truncated.next.test', b'dns-alias.next.test', b'dns-referral.next.test'):
            packet = query(host)
            response = reply(packet)
            self.assertEqual(response[:2], packet[:2])
            self.assertEqual(response[12:len(packet)], packet[12:])
        self.assertTrue(reply(query(b'dns-truncated.next.test'))[2] & 2)
        self.assertEqual(struct.unpack('!H', reply(query(b'dns-alias.next.test'))[6:8])[0], 1)
        self.assertEqual(struct.unpack('!H', reply(query(b'dns-referral.next.test'))[8:10])[0], 1)
        self.assertNotEqual(reply(query(b'dns-malformed.next.test'))[:2], query()[:2])
        self.assertEqual(reply(query(b'dns-timeout.next.test')), b'')

    def test_omitted_flow_does_not_block_other_actual_udp_queries(self):
        peer = Responder(0)
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                client.settimeout(0.05)
                destination = ('127.0.0.1', peer.port)
                client.sendto(query(b'dns-timeout.next.test'), destination)
                with self.assertRaises(socket.timeout): client.recvfrom(512)
                client.settimeout(1)
                for host in (b'next.test', b'dns-malformed.next.test', b'dns-truncated.next.test', b'dns-alias.next.test', b'dns-referral.next.test'):
                    packet = query(host)
                    client.sendto(packet, destination)
                    response, source = client.recvfrom(512)
                    self.assertEqual(source, destination)
                    self.assertEqual(response, reply(packet))
        finally:
            peer.close()
        self.assertEqual(peer.stats['dropped'], 1)
        self.assertEqual(peer.stats['answers'], 5)

    def test_all_truncations_compression_and_oversize_are_rejected(self):
        packet = query()
        for length in range(len(packet)):
            self.assertIsNone(reply(packet[:length]))
        self.assertIsNone(reply(packet + b'\0'))
        self.assertIsNone(reply(struct.pack('!6H', 1, 0x100, 1, 0, 0, 0) + b'\xc0\x0c\x00\x01\x00\x01'))
        self.assertIsNone(reply(bytes(513)))

    def test_actual_loopback_udp_and_bounded_shutdown(self):
        peer = Responder(0)
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                client.settimeout(1)
                packet = query()
                client.sendto(packet, ('127.0.0.1', peer.port))
                response, source = client.recvfrom(512)
                self.assertEqual(source, ('127.0.0.1', peer.port))
                self.assertEqual(response, reply(packet))
        finally:
            peer.close()
        self.assertGreaterEqual(peer.stats['answers'], 1)

    def test_intentional_loss_then_actual_udp_retry(self):
        peer = Responder(0, drop_first=True)
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
                client.settimeout(0.05)
                packet = query()
                destination = ('127.0.0.1', peer.port)
                client.sendto(packet, destination)
                with self.assertRaises(socket.timeout):
                    client.recvfrom(512)
                client.settimeout(1)
                client.sendto(packet, destination)
                response, _ = client.recvfrom(512)
                self.assertEqual(response, reply(packet))
        finally:
            peer.close()
        self.assertEqual(peer.stats['dropped'], 1)
        self.assertEqual(peer.stats['answers'], 1)

    def test_boot_flow_loss_counts_remain_exact_when_native_flows_follow(self):
        peer = Responder(0, drop_first=True)
        try:
            destination = ('127.0.0.1', peer.port)
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as boot, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as native:
                for client in (boot, native):
                    client.settimeout(0.03)
                    client.sendto(query(), destination)
                    with self.assertRaises(socket.timeout): client.recvfrom(512)
                    client.settimeout(1)
                    client.sendto(query(), destination)
                    self.assertEqual(client.recvfrom(512)[0], reply(query()))
                native.sendto(query(b'dns-timeout.next.test'), destination)
                native.settimeout(0.03)
                with self.assertRaises(socket.timeout): native.recvfrom(512)
        finally:
            peer.close()
        self.assertEqual(peer.stats['boot_dropped'], 1)
        self.assertEqual(peer.stats['boot_answers'], 1)
        self.assertEqual(peer.stats['boot_requests'], 2)
        self.assertEqual(peer.stats['dropped'], 3)
        self.assertEqual(peer.stats['answers'], 2)

    @unittest.skipUnless(hasattr(socket, 'SO_REUSEPORT'), 'reuseport unavailable')
    def test_concurrent_peers_keep_loss_and_replies_per_source_flow(self):
        first = Responder(0, drop_first=True)
        second = None
        try:
            second = Responder(first.port, drop_first=True)
            destination = ('127.0.0.1', first.port)
            # Keep sockets alive so the OS cannot recycle a source port. The
            # same flow changes question type, while retaining its first-drop
            # state on whichever responder owns the reuseport hash bucket.
            clients = [socket.socket(socket.AF_INET, socket.SOCK_DGRAM) for _ in range(8)]
            try:
                for client in clients:
                    client.settimeout(0.03)
                    packet = query()
                    client.sendto(packet, destination)
                    with self.assertRaises(socket.timeout):
                        client.recvfrom(512)
                    client.settimeout(1)
                    for kind in (1, 28):
                        packet = query(kind=kind)
                        client.sendto(packet, destination)
                        response, source = client.recvfrom(512)
                        self.assertEqual(source, destination)
                        self.assertEqual(response, reply(packet))
            finally:
                for client in clients:
                    client.close()
        finally:
            first.close()
            if second is not None:
                second.close()
        self.assertEqual(first.stats['dropped'] + second.stats['dropped'], 8)
        self.assertEqual(first.stats['answers'] + second.stats['answers'], 16)


if __name__ == '__main__':
    unittest.main()
