# Prism visual gallery

These screenshots come from completed framebuffer presentations in a running
q35/TCG/OVMF guest. The smoke harness launches or opens each view with QMP
keyboard input, captures the guest screen, and validates the image dimensions.
They document the current development build; they do not certify accessibility,
hardware display behavior, text shaping, or Next 1.0 completion. The current
qemu64 run passed 120 checks on image SHA256
`f4986796589aa9e5d75ad5eddbaf620a17613ccb35bbdfb66d7e17ca3a40fc0d`. Its report,
source captures, and related test evidence are retained in the [runtime and
Prism evidence](evidence/milestones/runtime-prism-2026-10-12/).

![Overview contact sheet of all twelve guest captures](images/prism-overview.png)

Regenerate the source captures with:

```sh
source tools/env.sh
bash tools/build.sh
python3 tools/boot_smoke.py --timeout 360 --capture-prism-gallery
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
it. Snap behavior is limited to the single current work area. The Calculator is
an expression utility rather than a complete scientific calculator.

## Home

![Prism Home with centered pinned apps and taskbar](images/prism-home.png)

## Terminal

![Next Terminal in the Prism desktop](images/prism-terminal.png)

## Files

![Prism Files panel](images/prism-files.png)

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
