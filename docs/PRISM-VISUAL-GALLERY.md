# Prism visual gallery

These screenshots come from completed framebuffer presentations in a running
q35/TCG/OVMF guest. The smoke harness launches or opens each view with QMP
keyboard input, captures the guest screen, and validates the image dimensions.
They document the current development build; they do not certify accessibility,
hardware display behavior, text shaping, or Next 1.0 completion. The exact
qemu64 run report and matching image hash are retained in the [runtime and Prism
evidence](evidence/milestones/runtime-prism-2026-10-12/).

![Overview contact sheet of all ten guest captures](images/prism-overview.png)

Regenerate the source captures with:

```sh
source tools/env.sh
bash tools/build.sh
python3 tools/boot_smoke.py --timeout 240 --capture-prism-gallery
```

The gallery includes the current six kernel-hosted first-party panels, plus a
second Settings capture taken after saving the Accessibility focus-outline
choice. Settings has 12 navigable categories: Appearance has active
theme/accent controls and Save/Revert, Accessibility has the saved
Standard/Strong focus-outline choice, System offers read-only diagnostics, and
the other nine categories remain informational with unavailable settings rows.
The Accessibility frame shows the keyboard path, saved state, and rendered
outline; it does not establish the screen reader, speech, magnification, or
other controls that remain unavailable. The Calculator is an expression
utility rather than a complete scientific calculator.

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

## Editor

![Prism Editor panel](images/prism-editor.png)

## Calculator

![Prism Calculator panel ready for an expression](images/prism-calculator.png)

## Keyboard shortcuts

![Prism keyboard shortcut guide](images/prism-shortcuts.png)

## Notifications

![Prism notification center](images/prism-notifications.png)
