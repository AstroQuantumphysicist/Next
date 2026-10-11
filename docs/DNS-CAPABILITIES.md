# Development DNS lookup capabilities

Ninth-wave source adds asynchronous native lookup calls 36–37. This is outside
the immutable eighth-wave image and requires separate guest validation. It is
a bounded UDP resolver adapter, not a recursive resolver, DNSSEC validator,
search-domain interface, or TCP DNS implementation.

`LookupCreate` takes name pointer in RDI, byte length in RSI, family in RDX
(A=1, AAAA=28), rights in R10, absolute monotonic deadline in R8, and zero in
R9. A zero deadline selects five seconds; explicit deadlines are bounded to
30 seconds. Names are canonical lowercase ASCII labels, at most 253 bytes,
with no empty labels, trailing dot, leading/trailing label hyphens, or implicit
case conversion. The complete name is copied and validated before allocation.
Rights may contain READ, DUPLICATE, TRANSFER and WAIT (mask `0x39`).

The runtime checks independently verified signed NETWORK declarations and
the immutable execution grant, plus the capability ceiling. It repeats the
NETWORK check on duplicate, IPC import/transfer, result access and wait. A
signed request for NETWORK does not grant it. No application supplies a
resolver address: this profile fixes `10.0.2.2:39099`. Four generation-checked,
reference-counted requests use internal local ports 49300–49303, separate
from connected application UDP sockets. DMA remains owned by the retained
network driver when a request closes or its process dies.

`LookupResult` takes a READ lookup capability in RDI, output pointer in RSI,
and capacity in RDX. Capacity must be 1–144 and the entire range writable;
invalid output returns InvalidArgument. A valid short range returns
BufferTooSmall without partial writes. Success returns 144 bytes:

| Offset | Field |
| --- | --- |
| 0 | u32 version=1 |
| 4 | u16 family (1 or 28) |
| 6 | u16 count (1–8) |
| 8 | u32 observed TTL seconds |
| 12 | u32 flags: bit 0 indicates cache hit |
| 16 | Eight 16-byte address slots |

Fields use little endian. IPv4 occupies the first four bytes of a slot with
the remaining bytes zero; IPv6 occupies all 16. Unused slots are zero. Results
are immutable and repeatable until the last capability closes. Retaining a
completed lookup does not extend the answer's TTL or assert current DNS
validity. Duplicates therefore retain the same terminal result independently.

Pending result access returns WouldBlock. ObjectWait with flags 0/1 polls or
parks on completion, requiring WAIT and NETWORK; success writes zero without
consuming the result. Writable flags are invalid for lookups. NxDomain and
NoData map to NotFound. Truncated responses, aliases and referrals map to
NotSupported; the adapter never fabricates successful TCP/alias resolution.
Omitted or malformed replies leave the request pending until TimedOut. Device
or lease failure terminates pending operations with an explicit error.

`sdk::Lookup` provides begin, result, wait, duplicate and close. The original
`dns-probe` fixture checks actual A/AAAA replies, cache use, rejected output,
duplicate lifetime, terminal outcomes and bounded request exhaustion/reuse.
The configured service worker checks that a NETWORK declaration with execution
grant zero cannot create a lookup. Host validation alone does not establish
guest execution of these new calls.
