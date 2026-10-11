# Immutable sixth-wave development milestone

Base image SHA256: `bf5faba563015d40c35f04dbfd126bb236fe5654c23ba4393aaeb35224872c8f`.

The exact 157-file source snapshot is preserved byte-for-byte; SHA256 `fdfd7dc5992743a9c7f1b95d5d215f4dd0b109c1b1534b0bdc20f0bdbe498533`. The only final source-manifest adjustment was the service-pair validator fix; compiled sixth implementation sources were preserved. `source-provenance.json` records this distinction. Seventh UDP/GPT/active-APIC work is outside this image.

Evidence includes 311 workspace Rust tests (73 core + 2 descriptor + 48 ABI + 94 checks + 20 FS + 18 package + 56 Prism), separately scoped installer 8 / storage 7 / Linux 28 / SDK 3 and Python checks; qemu64/max each 112 assertions; 30 loader refusals; two-boot persistence 7 checks with actual guest generation read-only recovery export/verification; runtime 6 checks; separately signed alternate two-worker service configuration 5 checks; 120 actual PS/2 latency samples. Separate suites can repeat primitives. Keys are excluded.

New guest proof covers PID1-only authenticated declarative configuration, exact worker identity/authority attenuation and dependency-ordered concurrent workers under an alternate signed configuration. Masked LAPIC countdown is calibrated against two bounded HPET windows; a masked running firmware timer is adopted/stopped, LVT/divider restored, and PIT remains scheduler. IOAPIC routing stays unchanged. Both CPU profiles show bounded UART queued output surviving stalls, accepted equals transmitted, pending zero and no rejected/absent bytes; this is not an unlimited lossless-output guarantee.

Retained observations:

- `observations/lapic-firmware-busy/`: earlier image `112baaf5758b81d215f8232d5fb3ae29ea93bf90fecbe96c39e72ecbff59d61d` rejected Busy firmware timer state. QMP/serial/source diagnostics remain retained; later final image passes after explicit masked-state adoption. This failure is not relabeled as final-image calibration proof.
- `observations/service-pair-frame-gate/`: an alternate signed image on the same base/payload timed out when its validator gated on an initial frame before complete Ready. Failed report/log/validator remain retained; corrected validator and separately identified alternate image pass. This is a validation-response ordering correction, not a service failure claim.

Inventory remains 108 partial, 87 pending, zero verified. Full Next 1.0 acceptance and physical profiles remain unverified. Latency is fixed-order host-observed TCG timing, not hardware/IRQ/vblank certification.
