# Validation record

The final source wave passed host validation and built the signed development image used for R10:

- `bash tools/test.sh` — passed (exit 0), including workspace validation, formatting and strict lint gates.
- `source tools/env.sh && bash tools/build.sh` — passed (exit 0); produced the 68,157,440-byte signed image recorded in `build-manifest.json`.

The R10 QEMU boot smoke passed **138 checks** (exit 0) on q35/TCG/OVMF with CPU `qemu64`, 256 MiB, one configured vCPU and `vga=std`. The image SHA-256 is `64d07c2ff4b53c3e14dafdb23ed28b655bafc98eab9543fe099229910f302d10`. AP startup was disabled, AP reservations remained intact, and scheduler work ran only on the BSP. The image is not included in this bundle.

The run executed the native one-shot monotonic timer fixture twice. Both runs emitted `TIMER PROBE: monotonic expiry, zero/absolute deadlines, cancellation, rights, queued transfer, stale handles and quotas passed` and exited 58. The guest report records both zero and absolute deadline checks. Home search selected the accessible `2+3*4 = 14` result and Enter opened Calculator. The Linux cwd/openat fixture exited 56; the Linux timefile fixture exited 57 after checking `gettimeofday` ordered copyout and `ftruncate`. The captured audio passed the report's 48 kHz stereo, channel and 1 kHz waveform checks.

The visual set contains 20 1280×800 QEMU frames from this image: Home, app search and arithmetic search; Terminal, Files, Settings, Editor, Calculator, System Monitor, shortcuts and notifications; saved accessibility/text-size states; five Files root/nested/cursor/dual-pane/copy interaction captures; and a quadrant snap. `capture-manifest.json` links each PNG to its R10 guest PPM SHA-256.

The requirement inventory remains 195 entries: **74 pending, 121 partial, 0 verified**; `product_ready=false`.

## Scope limits

This is q35/TCG evidence, not physical-hardware validation. One vCPU was configured; APs remained stopped and this run does not establish SMP scheduling. The calibrated LAPIC countdown is bounded timer-source evidence only: it does not establish a high-resolution clock, high-resolution scheduling guarantees or suspend/resume behavior. There is no suspend/resume result in this run. The timer capability has bounded capacity and one-shot monotonic deadlines; it does not provide periodic timers, UTC alarms, high-resolution guarantees or SMP synchronization. The Prism panels run in the kernel and do not establish a userspace compositor or full accessibility services. These results add partial evidence only and do not imply Next 1.0 completion.
