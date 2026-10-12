# Feature-wave evidence — current development image

All guest reports in this bundle use the same signed development image,
SHA-256 `f4986796589aa9e5d75ad5eddbaf620a17613ccb35bbdfb66d7e17ca3a40fc0d`.
The qemu64 smoke passed 120 checks and captured the current Prism gallery after
completed guest frames. Its three current SMP-topology profiles each passed 119
checks: QMP reports 2/4/8 CPU objects and ACPI/MADT reports the same enabled
count. All profiles keep AP startup disabled and run the scheduler on the BSP;
they do not test cross-core execution, SMP scheduling, or physical hardware.

## Guest evidence

- `qemu64/boot-smoke.json` records the 120-check run, deployment signature and
  image digests, runtime checks, screenshot dimensions, audio capture, and the
  actual saved Settings and snap interactions. Raw serial and QEMU logs, WAV,
  and a guest framebuffer PNG are alongside it.
- `smp/smp-{2,4,8}/boot-smoke.json` and compressed serial logs record each
  current-image topology run. ACPI CPU counts match QMP at each profile; AP
  reservations remain retained and no AP online markers appear.
- The [Prism gallery](../../../../PRISM-VISUAL-GALLERY.md) and
  [`docs/images`](../../../../images/) contain 12 lossless guest captures from
  the same current image: Home, Terminal, Files, Monitor, Settings, saved
  Accessibility focus outline, saved 125% Settings text size, Editor,
  Calculator, keyboard shortcuts, notifications, and a BottomLeft quarter snap.
  The smoke validates completed frames and dimensions; it does not certify
  visual accessibility or display hardware.

## Host validation

`validation/workspace-validation.txt` records the passing full
`bash tools/test.sh` run, including strict Clippy and formatting checks. Focused
logs retain these results:

- `backup-tests.txt`: 53 tests for a bounded test/host-gated archive codec and
  journal restore bridge, including complete old-or-new recovery across 69
  write/flush fault points. There is no destination picker, encryption,
  signature, snapshot history, or Prism backup workflow.
- `package-host-tests.txt`: 47 package host-tool tests, including pinned-root
  protection against replacing an ancestor after opening the install root.
  Guest package installation and hostile mutation inside the private root are
  outside this guarantee.
- `network-dma-tests.txt` and `network-dma-host.json`: 78 production-source
  network tests including bounded DHCPv6 parsing/client-state transitions.
  This does not send guest DHCPv6 packets or install addresses, DAD, or routes.
- `linux-fs-contract-tests.txt`: 7 host contract tests for Linux `getpid`,
  `uname`, and `lseek`; the new calls do not yet have guest Linux fixtures.
- `acpi-topology-tests.txt`: 10 host parser tests, including exact enabled CPU
  identities and synthetic 1/2/4/8-entry tables. These checks do not start APs.
- `prism-tests.txt`: 212 Prism tests for current shell, Settings, snapping, and
  capture policy behavior. `capture-privacy-tests.txt` isolates 6 tests for the
  host-only consent model. The policy does not receive screen pixels and is not
  integrated into a compositor or capture UI.

The ledger records **74 pending, 121 partial, and 0 verified** requirements;
`product_ready` remains false. This wave adds bounded foundations and evidence,
not the complete Next 1.0 contract.
