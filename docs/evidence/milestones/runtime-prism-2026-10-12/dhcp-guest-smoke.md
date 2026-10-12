# Guest DHCP renewal and expiry evidence

The current Next guest image booted in QEMU q35/TCG and used a deterministic Ethernet peer over QEMU's socket netdev. The peer granted a short lease (T1 12 seconds, T2 30 seconds), acknowledged the first renewal with a refreshed 40-second lease, dropped the next T1 requests, answered the broadcast T2 rebind from a different DHCP server, then returned a NAK at the next T1. After the guest reacquired its lease, the peer dropped T1 and T2 traffic through expiry.

The guest completed the full sequence:

- 2.145s: `initial DHCP discovery`
- 2.147s: `initial-ack`
- 14.252s: `t1-renewal` (unicast)
- 14.252s: `t1-ack-refresh`
- 26.260s: `t1-renewal-dropped` (unicast)
- 27.270s: `t1-renewal-dropped` (unicast)
- 29.280s: `t1-renewal-dropped` (unicast)
- 33.290s: `t1-renewal-dropped` (unicast)
- 41.300s: `t1-renewal-dropped` (unicast)
- 44.260s: `t2-rebind` (broadcast)
- 44.260s: `t2-ack-refresh` (server 0a000203)
- 56.278s: `t1-renewal-nak` (server 0a000203)
- 56.289s: `NAK recovery discovery`
- 56.289s: `reacquire-ack`
- 96.306s: `lease-expiry-discovery`


Measured intervals were 12.105s from initial ACK to T1, 12.008s from refreshed ACK to the next T1, 30.008s from refreshed ACK to T2, 12.018s from rebind ACK to NAK, 0.011s from NAK to discovery restart, and 40.017s from reacquisition ACK to expiry restart.

The JSON trace records the exact image SHA-256 (`e69aa9ca8de049c2a12d048e28e56525a0c352992b2385b200daba70de4ab484`), QEMU profile, source hashes, lease policy, timer checks, and limits. The peer-driven run proves the guest path in QEMU TCG; it does not claim hardware certification or broad DHCP interoperability.
