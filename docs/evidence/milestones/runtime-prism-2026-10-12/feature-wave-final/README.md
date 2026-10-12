# Final feature-wave evidence

All guest evidence in this bundle is bound to the same signed development
image, SHA-256
`8902846a7a484186361520c6c626846479710faef2afa8729bdad664e66825f3`.

`boot-smoke.json` records 125 passing qemu64 assertions on q35/TCG/OVMF with
one configured vCPU. The smoke exercises the signed boot chain, kernel/runtime
checks, a saved Accessibility Settings interaction, all 12 main Prism gallery
frames, independent Files `/native` and `/native/alpha` tabs, an I-beam pixel
check, and Linux `getpid`, `uname`, and `lseek` guest fixtures (exit statuses
51, 52, and 53). `images/` contains the 15 corresponding lossless PNG views:
the 12 main frames plus Files root, nested, and cursor captures. The raw serial
stream is gzip-compressed; the smoke report records exact screenshot paths and
dimensions.

`dhcpv6-guest-smoke.json` is a second QEMU run on the same image with a
deterministic Ethernet peer. It drops the first Solicit and Request, observes
stable transaction IDs and Elapsed Time advancing from 0 to 100 centiseconds
on retransmission, checks the Request's zero T1/T2 and zero IAADDR lifetimes,
then checks that the matching Reply supplies server timers 20/40 seconds and
preferred/valid lifetimes 40/60 seconds. The kernel retains a lease candidate
only. It does not install an IPv6 address, perform DAD, configure routes, or
use the candidate for sockets. The peer capture has no unexpected or rejected
DHCPv6 events. The DHCPv6 run retains the final guest serial tail rather than
the complete raw serial stream.

The full `bash tools/test.sh` run and `bash tools/build.sh` succeeded after the
DHCPv6 correction. The network host harness passed 83 production-source tests;
its report records source hashes in `network-dma-host.json`. The image remains
a development build: 195 requirements are tracked, with 74 pending, 121
partial, 0 verified, and `product_ready=false`. The one-vCPU TCG run is not
SMP scheduling or hardware certification.

`SHA256SUMS` covers every file in this directory. The manifest hashes identify
artifacts; they do not independently prove behavior beyond the scoped reports.
