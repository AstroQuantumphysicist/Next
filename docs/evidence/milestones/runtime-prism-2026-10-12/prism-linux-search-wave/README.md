# Prism and Linux profile evidence — 2026-10-12

This bundle records the signed qemu64 development image with SHA-256 `64d07c2ff4b53c3e14dafdb23ed28b655bafc98eab9543fe099229910f302d10` and its passing 138-check QEMU run. The image is not duplicated here. `build-manifest.json` records the signed EFI members; `source-manifest.json` hashes the changed kernel, timer, SDK, Prism, fixture and host-test/build inputs, along with the guest ABI note.

`capture-manifest.json` binds twenty current QEMU framebuffer captures to their R10 PPM hashes and PNG hashes. The twenty captures are fourteen Prism gallery views, five Files interaction frames, and one snap frame. `images/` contains these full-size images and their contact sheet. `boot-smoke.json`, `serial.log.gz`, `qemu-smoke.log`, and `audio.wav` preserve the guest report, serial trace, QEMU log and audio probe output. `validation.md` records host checks, exact guest results and scope limits.

The image ran in q35/TCG/OVMF with CPU `qemu64`, one configured vCPU and 256 MiB. AP startup remained disabled and scheduler work ran only on the BSP. This is a development build; it is not Next 1.0, a hardware certification, an SMP result or evidence of a high-resolution clock.
