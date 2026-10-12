# Prism visual gallery

These screenshots come from completed framebuffer presentations in a running
q35/TCG/OVMF guest. The smoke harness launches or opens each view with QMP
keyboard input, captures the guest screen, and validates the image dimensions.
They document the current development build; they do not certify accessibility,
hardware display behavior, text shaping, or Next 1.0 completion. The exact
qemu64 run report and matching image hash are retained in the [runtime and Prism
evidence](evidence/milestones/runtime-prism-2026-10-12/).

![Overview contact sheet of all nine guest captures](images/prism-overview.png)

Regenerate the source captures with:

```sh
source tools/env.sh
bash tools/build.sh
python3 tools/boot_smoke.py --timeout 240 --capture-prism-gallery
```

The gallery includes the current six kernel-hosted first-party panels. Settings
shows 12 navigable categories: Appearance has active theme/accent controls and
Save/Revert, System offers read-only diagnostics, and the other ten pages are
informational with disabled settings rows. The capture shows the rendered
Appearance page; it does not establish controls that remain unavailable. The
Calculator is an expression utility rather than a complete scientific
calculator.

## Home

![Prism Home with centered pinned apps and taskbar](images/prism-home.png)

## Terminal

![Next Terminal in the Prism desktop](images/prism-terminal.png)

## Files

![Prism Files panel](images/prism-files.png)

## System Monitor

![Prism System Monitor panel](images/prism-monitor.png)

## Settings

![Prism Settings with 12 categories, active Appearance controls, and read-only or informational sections](images/prism-settings.png)

## Editor

![Prism Editor panel](images/prism-editor.png)

## Calculator

![Prism Calculator panel ready for an expression](images/prism-calculator.png)

## Keyboard shortcuts

![Prism keyboard shortcut guide](images/prism-shortcuts.png)

## Notifications

![Prism notification center](images/prism-notifications.png)
