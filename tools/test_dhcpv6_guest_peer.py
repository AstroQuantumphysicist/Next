import struct
import unittest

import dhcp_guest_smoke as smoke


class FakeConnection:
    def __init__(self):
        self.frames = []

    def sendall(self, data):
        self.frames.append(data)


class Dhcpv6PeerTests(unittest.TestCase):
    def setUp(self):
        self.peer = smoke.EthernetPeer(None)
        self.peer.connection = FakeConnection()

    def guest_frame(self, kind, xid, options):
        payload = smoke.make_dhcpv6_message(kind, xid, options)
        return smoke.dhcpv6_udp_frame(
            bytes.fromhex("333300010002"), smoke.CLIENT_MAC, bytes(16),
            smoke.DHCPV6_SERVERS, 546, 547, payload,
        )

    def solicit(self, xid, elapsed=0, ia=None, options=None):
        if options is None:
            options = [
                (1, smoke.DHCPV6_CLIENT_DUID),
                (8, struct.pack("!H", elapsed)),
                (3, smoke.DHCPV6_IA_NA_SOLICIT if ia is None else ia),
            ]
        return self.guest_frame(1, xid, options)

    def request(self, xid, elapsed=0, ia=None, options=None):
        if options is None:
            options = [
                (1, smoke.DHCPV6_CLIENT_DUID),
                (2, smoke.DHCPV6_SERVER_DUID),
                (8, struct.pack("!H", elapsed)),
                (3, smoke.dhcpv6_request_ia(smoke.DHCPV6_IAID) if ia is None else ia),
            ]
        return self.guest_frame(3, xid, options)

    def sent_message(self, index):
        framed = self.peer.connection.frames[index]
        length = struct.unpack_from("!I", framed)[0]
        frame = framed[4:]
        self.assertEqual(len(frame), length)
        return smoke.dhcpv6_message(frame, (547, 546))

    def test_peer_drops_then_answers_bound_solicit_and_request_retries(self):
        solicit_xid = b"\x01\x02\x03"
        request_xid = b"\x04\x05\x06"
        self.peer._handle_dhcpv6(self.solicit(solicit_xid))
        self.peer._handle_dhcpv6(self.solicit(solicit_xid, elapsed=101))

        advertise = self.sent_message(0)
        self.assertEqual(advertise["kind"], 2)
        self.assertEqual(advertise["xid"], solicit_xid)
        self.assertEqual(advertise["options"][1], smoke.DHCPV6_CLIENT_DUID)
        self.assertEqual(advertise["options"][2], smoke.DHCPV6_SERVER_DUID)
        self.assertEqual(struct.unpack_from("!I", advertise["options"][3])[0], smoke.DHCPV6_IAID)

        self.peer._handle_dhcpv6(self.request(request_xid))
        self.peer._handle_dhcpv6(self.request(request_xid, elapsed=102))

        reply = self.sent_message(1)
        self.assertEqual(reply["kind"], 7)
        self.assertEqual(reply["xid"], request_xid)
        self.assertEqual(reply["options"][1], smoke.DHCPV6_CLIENT_DUID)
        self.assertEqual(reply["options"][2], smoke.DHCPV6_SERVER_DUID)
        reply_ia = reply["options"][3]
        self.assertEqual(struct.unpack_from("!III", reply_ia),
                         (smoke.DHCPV6_IAID, 20, 40))
        self.assertEqual(reply_ia[12:16], b"\x00\x05\x00\x18")
        self.assertEqual(reply_ia[16:32], smoke.DHCPV6_ADDRESS)
        self.assertEqual(struct.unpack_from("!II", reply_ia, 32), (40, 60))
        events = self.peer.snapshot()
        self.assertEqual(
            [event["event"] for event in events],
            ["dhcpv6-solicit-dropped", "dhcpv6-solicit-retry", "dhcpv6-advertise-sent",
             "dhcpv6-request-dropped", "dhcpv6-request-retry", "dhcpv6-reply-sent"],
        )
        solicit_initial, solicit_retry = events[:2]
        request_initial, request_retry = events[3:5]
        self.assertEqual(solicit_initial["xid"], solicit_retry["xid"])
        self.assertGreater(solicit_retry["elapsed_centiseconds"],
                           solicit_initial["elapsed_centiseconds"])
        self.assertEqual(request_initial["xid"], request_retry["xid"])
        self.assertGreater(request_retry["elapsed_centiseconds"],
                           request_initial["elapsed_centiseconds"])
        self.assertEqual(smoke.dhcpv6_exchange_errors(events), [])

    def test_peer_rejects_changed_retry_ids_elapsed_time_and_ia_shape(self):
        solicit_xid = b"\x11\x22\x33"
        request_xid = b"\x44\x55\x66"
        self.peer._handle_dhcpv6(self.solicit(solicit_xid))
        self.peer._handle_dhcpv6(self.solicit(b"\x11\x22\x34", elapsed=100))
        self.peer._handle_dhcpv6(self.solicit(solicit_xid, elapsed=0))
        malformed_ia = smoke.DHCPV6_IA_NA_SOLICIT + b"\x00\x00"
        self.peer._handle_dhcpv6(self.solicit(solicit_xid, elapsed=100, ia=malformed_ia))
        self.peer._handle_dhcpv6(self.solicit(solicit_xid, elapsed=100))

        self.peer._handle_dhcpv6(self.request(request_xid))
        self.peer._handle_dhcpv6(self.request(b"\x44\x55\x67", elapsed=100))
        self.peer._handle_dhcpv6(self.request(request_xid, elapsed=0))
        malformed_request_ia = bytearray(smoke.dhcpv6_request_ia(smoke.DHCPV6_IAID))
        malformed_request_ia[-1] ^= 1
        self.peer._handle_dhcpv6(self.request(request_xid, elapsed=100, ia=bytes(malformed_request_ia)))
        self.peer._handle_dhcpv6(self.request(request_xid, elapsed=100))

        events = self.peer.snapshot()
        kinds = [event["event"] for event in events]
        self.assertIn("dhcpv6-client-xid-rejected", kinds)
        self.assertIn("dhcpv6-client-timing-rejected", kinds)
        self.assertIn("dhcpv6-client-ia-rejected", kinds)
        self.assertIn("dhcpv6-request-xid-rejected", kinds)
        self.assertIn("dhcpv6-request-timing-rejected", kinds)
        self.assertIn("dhcpv6-request-ia-rejected", kinds)
        self.assertIn("dhcpv6-advertise-sent", kinds)
        self.assertIn("dhcpv6-reply-sent", kinds)
        self.assertEqual(len(smoke.dhcpv6_exchange_errors(events)), 6)

    def test_malformed_checksum_duid_and_truncated_tlv_are_rejected(self):
        valid = self.solicit(b"\x09\x08\x07")
        self.assertIsNotNone(smoke.dhcpv6_message(valid))
        corrupt = bytearray(valid)
        corrupt[-1] ^= 1
        self.assertIsNone(smoke.dhcpv6_message(corrupt))
        self.peer._handle(bytes(corrupt))

        wrong_duid = self.solicit(
            b"\x09\x08\x07",
            options=[(1, b"\x00\x03\x00\x01wrong"),
                     (8, b"\x00\x00"), (3, smoke.DHCPV6_IA_NA_SOLICIT)],
        )
        self.peer._handle(wrong_duid)

        truncated = self.solicit(
            b"\x09\x08\x07",
            options=[(1, smoke.DHCPV6_CLIENT_DUID), (8, b"\x00\x00"),
                     (3, smoke.DHCPV6_IA_NA_SOLICIT)],
        )[:-1]
        self.assertIsNone(smoke.dhcpv6_message(truncated))
        self.peer._handle(truncated)
        events = self.peer.snapshot()
        self.assertEqual(
            [event["event"] for event in events],
            ["dhcpv6-malformed-client-frame", "dhcpv6-client-identity-rejected",
             "dhcpv6-malformed-client-frame"],
        )
        self.assertEqual(len(smoke.dhcpv6_exchange_errors(events)), 3)

    def test_non_dhcpv6_traffic_is_ignored_but_bad_client_tuple_fails(self):
        other_udp = smoke.dhcpv6_udp_frame(
            bytes.fromhex("333300010002"), smoke.CLIENT_MAC, bytes(16),
            smoke.DHCPV6_SERVERS, 546, 9999, b"not DHCPv6",
        )
        icmpv6 = bytearray(40 + 8)
        icmpv6[0] = 0x60
        struct.pack_into("!HBB", icmpv6, 4, 8, 58, 255)
        icmpv6[24:40] = bytes.fromhex("ff020000000000000000000000000001")
        ordinary_ipv6 = smoke.ethernet(
            bytes.fromhex("333300000001"), smoke.CLIENT_MAC, 0x86DD, bytes(icmpv6),
        )
        self.assertFalse(smoke.is_dhcpv6_client_tuple(other_udp))
        self.assertFalse(smoke.is_dhcpv6_client_tuple(ordinary_ipv6))
        self.peer._handle(other_udp)
        self.peer._handle(ordinary_ipv6)
        self.assertEqual(self.peer.snapshot(), [])

        malformed_dhcpv6 = bytearray(self.solicit(b"\x09\x08\x07"))
        malformed_dhcpv6[-1] ^= 1
        self.assertTrue(smoke.is_dhcpv6_client_tuple(malformed_dhcpv6))
        self.peer._handle(bytes(malformed_dhcpv6))
        self.assertEqual(
            [event["event"] for event in self.peer.snapshot()],
            ["dhcpv6-malformed-client-frame"],
        )


if __name__ == "__main__":
    unittest.main()
