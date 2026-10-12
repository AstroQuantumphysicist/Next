# ObjectWaitMany guest acceptance

Command: `source tools/env.sh && python3 tools/boot_smoke.py --cpu qemu64 --timeout 180`

The signed QEMU image passed 117 assertions under q35/TCG with qemu64, 256 MiB, and one CPU. Its SHA-256 is `9984f82ad71b67b3d8992e8e222d9e20739f0565ba0d579d0b877ae921653395`. The complete runner report is in [`object-wait-many-guest.json`](object-wait-many-guest.json), and the preserved serial log is in [`object-wait-many-serial.log`](object-wait-many-serial.log).

The original signed `event-wait.elf` guest fixture receives two attenuated event capabilities over IPC. It verifies that a blocking wait-many times out and removes its wait registrations, then acknowledges readiness. The supervisor starts the second wait and signals only input index 1. The guest confirms that polling unsignaled events returns `WouldBlock`, the blocking call returns index 1 after signal, a reset after delivery does not erase the completion, and a later poll does not consume another signal. The same run gates on the exact guest log marker.

This is integrated evidence for the bounded event-only syscall 45 path. It does not validate wait-many over process, surface, DNS or datagram objects, SMP wake races, or the full Next 1.0 synchronization contract.
