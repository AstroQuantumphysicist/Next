# Two-vCPU QEMU topology and BSP-only run

This report uses the same signed image SHA-256 `b79790f446b5bde892fbde78d63e3a177d558d2d1319556680d9f48f24efb8ce` as the final qemu64 gallery. QMP reports two configured CPUs and ACPI/MADT discovery returns two enabled CPU records. The report explicitly records `ap_startup_enabled=false` and scheduler scope `bootstrap CPU only`; the AP trampoline and scratch remain reserved. This validates boot/topology behavior with a two-vCPU virtual configuration, not AP execution, SMP scheduling, cross-core synchronization, or 2/4/8-core stress.
