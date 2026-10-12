# Clock, notification, and Prism evidence — 2026-10-12

This bundle records the signed qemu64 development image with SHA-256 `6b245fa85c2c49ad1da63cf7d6fdfb813e3d351c3275217c736d7b1c43b1da15` and its passing 138-check QEMU run. The image is not duplicated here. `build-manifest.json` hashes its signed EFI members; `source-manifest.json` binds the changed kernel, Linux fixture, package, Prism, and test/build source files.

`capture-manifest.json` binds twenty completed QEMU framebuffer captures to guest PPM hashes and PNG hashes. Fourteen Prism gallery views, five Files interaction captures, and one snap frame appear both in `images/` and `docs/images/`; the overview is a labeled contact sheet. `boot-smoke.json`, `serial.log.gz`, `qemu-smoke.log`, and `audio.wav` preserve the guest report and raw run evidence.

The Linux clock fixture ran twice and exited 43 both times after exercising `clock_gettime`, `clock_getres`, realtime and monotonic reads, monotonic sleep, error bounds, and copyout behavior. The guest uses a validated HPET counter for monotonic reads when available, while PIT remains the scheduler/deadline source. The notification capture shows the bounded session-only Block app control for the selected process publisher. Host tests cover exact publisher policy behavior and package installer lock recovery.

The image ran under q35/TCG/OVMF with CPU `qemu64`, one configured vCPU and 256 MiB. AP startup remained disabled and scheduler work ran only on the BSP. This is development evidence, not Next 1.0, physical-hardware certification, an SMP result, or proof of high-resolution scheduling.
