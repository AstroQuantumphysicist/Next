The file portal is a bounded protocol model for a future trusted file chooser.
It does not provide guest syscalls, a UI, filesystem access, or installed
capabilities. Its caller supplies synchronization, a monotonic clock, trusted
chooser authority, and the runtime's authenticated immutable process identity.
Never construct that identity from user-provided package names or manifest
permission declarations.

`ProcessIdentity` binds an authenticated digest to PID and process lifetime
generation. `Portal<N>` bounds total slots and per-process outstanding requests.
`RequestId` contains a slot generation; stale IDs cannot approve, consume or
release a reused slot. Exhausted generations permanently retire the slot.
Requests expose only typed read-only/read-write access and an absolute deadline.
They do not give the requester ambient path authority.

The trusted UI chooses a canonical `/native/` path of at most 31 bytes and may
attenuate read-write to read-only. It must approve the exact displayed recipient
identity and lifetime. Unsupported namespaces and broader rights fail without
changing the pending request. No manifest bits are accepted as consent.

| Current state | Operation | Result |
| --- | --- | --- |
| Pending | Trusted approval | Approved |
| Pending | Trusted denial | Denied |
| Pending or Approved | Recipient cancellation | Cancelled |
| Pending or Approved | Deadline reached | Expired |
| Approved | Bound recipient consumption | Consumed, move-only grant returned |
| Denied, Cancelled, Expired or Consumed | Bound recipient acknowledgement | Slot freed, generation incremented |
| Any occupied state | Trusted process death | Choice cleared, ID invalidated, slot freed |

Terminal results remain queryable until acknowledged and count against quotas;
this prevents unlimited terminal-state accumulation. `pending(now)` and every
time-taking operation apply expiry before acting. A regressing clock fails
closed. Consumed replies cannot be replayed or approved again.

The runtime must validate that the grant's exact recipient is still live when
installing a file capability, and install directly into that recipient's table.
The move-only grant is not a bearer token accepted from user memory. A failed
filesystem open can consume the request without granting anything; retry needs
a fresh chooser request. Process teardown must call `process_died` and cancel
pending capability installation. This model does not automatically revoke an
already installed file object, implement a durable consent database, or imply
production permission-broker support.

The kernel `kernelportal` adapter now applies a four-request total and one-request
per-process limit, with absolute deadlines at most 30 seconds ahead. Requests use
typed wire access 1 (read-only) or 3 (read/write). Trusted UI approval opens the
chosen existing native file without create/truncate and retains its exact open
inode in a bounded approval pool. Renaming, unlinking, or recreating the selected
path before recipient polling cannot redirect consent to a replacement inode.
The grant follows the originally selected file's current contents.

The runtime peeks the exact approved, attenuated rights and reserves a recipient
capability slot before consuming. Consumption transfers the held open file and
reclaims the request. Handle exhaustion leaves approval intact; failed approval
opens leave the request pending. Cancellation closes a held approval immediately
and retains a Cancelled result until one poll returns it; denied and expired
results similarly return once and release their request quota. Repeated cancel
of an already terminal result may acknowledge it without another poll. Expiry
cleanup also runs while checking the UI's pending queue, even when that queue is
empty; process teardown closes all held approvals for the exact lifetime.

Host adapter tests use the actual shared-inode `Files` implementation with a
modeled atomic namespace. They cover rename/unlink/recreation after consent,
attenuation, capacity failures, expiry, cancellation, process death, and recipient
confusion. Guest dialog/syscall validation and production consent policy remain
separate integration gates; these tests alone do not establish either.
