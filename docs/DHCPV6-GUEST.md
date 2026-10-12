# Bounded guest DHCPv6 exchange

The development virtio Ethernet path can run one small DHCPv6 exchange after
IPv4 lease acquisition. It sends a Solicit from `::` to `ff02::1:2`, accepts a
checksummed server response from a link-local source, binds the Advertise to the
client DUID and IAID, and sends Request with the selected server DUID and exact
offered IA_NA addresses. The client sets Request IA_NA T1/T2 and IAADDR
preferred/valid lifetimes to zero, leaving those values for the server to choose.
A matching Reply preserves the server's lease timers and lifetimes as a bounded
candidate.

Each Solicit and Request transaction retransmits at 1, 2, and 4 seconds, then
stops after one final 8-second response window. Retries update the elapsed-time
option while keeping the transaction ID stable. Wrong transaction IDs, DUIDs,
IAIDs, servers, packet checksums, malformed TLVs, addresses, and lifetimes are
ignored or rejected, and unsolicited Rapid Commit options are rejected because
the client did not request that mode. Unknown IPv6 UDP tuples and extension-
header chains remain unsupported.

The candidate is logged as `HELIX DHCPV6 CANDIDATE PASS`. The kernel does not
install its addresses, perform Duplicate Address Detection, configure routers,
or use it for sockets. T1/T2 renewal and rebind packet exchanges are still not
wired into the guest; their state transitions are covered by host tests only.
General IPv6 sockets, SLAAC address creation, neighbor learning, route
management, DNS over IPv6, mDNS, and non-virtio device coverage remain outside
this slice.

Host parser, checksum, encoder, malformed-frame, identity, and retry tests run
through `python3 tools/test-network-dma.py`; the deterministic peer parser also
has two Python fixture tests in the normal unit-test discovery. The focused
QEMU check is `python3 tools/dhcp_guest_smoke.py --dhcpv6-only
--image build/next-dev.img`. It drops the first Solicit and Request to exercise
real guest retransmission and reports each observed exchange. The full script
without `--dhcpv6-only` also runs the longer DHCPv4 T1/T2/expiry test. Results
are development-profile checks, not hardware or general network
interoperability evidence.

The final feature-wave QEMU report is in
[`feature-wave-final/dhcpv6-guest-smoke.json`](evidence/milestones/runtime-prism-2026-10-12/feature-wave-final/dhcpv6-guest-smoke.json)
and is bound to image SHA256
`8902846a7a484186361520c6c626846479710faef2afa8729bdad664e66825f3`. It
observed the selected address `2001:db8:1::9`; the Reply supplied T1/T2 20/40
and preferred/valid lifetimes 40/60 seconds. These values remain a candidate
and do not activate IPv6 networking in the guest.
