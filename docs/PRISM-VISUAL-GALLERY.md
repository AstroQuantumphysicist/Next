# Prism visual gallery

These screenshots come from completed framebuffer presentations in a running
q35/TCG/OVMF guest. The smoke harness launches or opens each view with QMP
keyboard input, captures the guest screen, and validates the image dimensions.
They document the current development build; they do not certify accessibility,
hardware display behavior, text shaping, or Next 1.0 completion. The current
qemu64 run passed 128 checks on image SHA256
`1b226f5ee89a59656cf74a673ea381f7d56e9dbe764bb852c75d224a902d6c6b`. The report,
capture hashes, and related test evidence are retained in the [runtime and
Prism evidence](evidence/milestones/runtime-prism-2026-10-12/fstat-getdents-files-r3/).
Prism uses Microsoft Selawik, an open-source metric-compatible replacement
for Segoe UI; it is not Segoe UI Variable. The raster atlas, provenance, and
unsupported shaping limits are listed in the [font notices](third-party/README.md#font-assets).

![Overview contact sheet of all sixteen guest captures](images/prism-overview.png)

Regenerate the source captures with:

```sh
source tools/env.sh
bash tools/build.sh
python3 tools/boot_smoke.py --cpu qemu64 --timeout 360 --capture-prism-gallery \
  --image build/next-dev.img --output-dir build/fstat-getdents-files-r3
```

The gallery includes all six kernel-hosted first-party panels and two additional
Settings frames captured after saving the Strong focus outline and 125% Settings
text size. Settings has 12 navigable categories: Appearance has active
theme/accent controls and Save/Revert, Accessibility exposes the two saved
preferences, System offers read-only diagnostics, and the other nine categories
remain informational with unavailable settings rows. The larger text applies
only within Settings; it does not establish screen reader, speech,
magnification, or other services that remain unavailable. The quarter-snap
frame captures Terminal in the BottomLeft work-area quadrant with Editor behind
it. Snap behavior is limited to the single current work area. The 16-image set
also includes guest-created Files root and nested tabs, two panes that retain
independent paths, and the I-beam rendered over the editable location field.
The smoke switches between tabs and panes, checks path isolation, and checks the
cursor pixels. The Settings gallery flow selects Accessibility preferences,
saves them, and confirms the accepted save request. The Calculator is an
expression utility rather than a complete scientific calculator.

## Home

![Prism Home with centered pinned apps and taskbar](images/prism-home.png)

## Terminal

![Next Terminal in the Prism desktop](images/prism-terminal.png)

## Files

![Prism Files panel](images/prism-files.png)

The following three frames come from the current signed-image guest and document
the separate Files tab views and cursor state exercised by the smoke.

### Files tabs: root view

![Prism Files with the /native root tab active while a second tab retains /native/alpha](images/prism-files-tabs-root.png)

### Files tabs: nested view

![Prism Files with /native/alpha active while the /native root tab remains open](images/prism-files-tabs-nested.png)

### Files location I-beam cursor

![Prism Files showing the I-beam pointer over its editable location field](images/prism-files-tabs-ibeam.png)

![Prism Files with independent left and right panes at different paths](images/prism-files-dual-pane.png)

The guest smoke changes each pane separately and confirms that navigation in
one pane retains the other pane's path.

## System Monitor

![Prism System Monitor panel](images/prism-monitor.png)

## Settings

![Prism Settings Appearance page with 12 categories and active theme/accent controls](images/prism-settings.png)

## Settings Accessibility

![Prism Settings with the saved Strong keyboard focus outline preference](images/prism-settings-accessibility.png)

## Settings text size

![Prism Settings after saving the 125% Settings-only text-size preference](images/prism-settings-text-scale.png)

## Editor

![Prism Editor panel](images/prism-editor.png)

## Calculator

![Prism Calculator panel ready for an expression](images/prism-calculator.png)

## Keyboard shortcuts

![Prism keyboard shortcut guide](images/prism-shortcuts.png)

## Notifications

![Prism notification center](images/prism-notifications.png)

## Quarter snapping

![Terminal snapped to the BottomLeft quadrant over the Editor window](images/prism-snap-quadrant.png)

For a captioned walkthrough of these frames, see the [Prism visual tour](PRISM-VISUAL-TOUR.md).
