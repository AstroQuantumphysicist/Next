# Prism visual gallery

These twenty screenshots are current captures from completed framebuffer presentations in a running q35/TCG/OVMF guest. The QMP smoke checked Home search selections through the guest accessibility snapshot and validated each captured frame. The signed qemu64 development image passed 138 checks with SHA256 `64d07c2ff4b53c3e14dafdb23ed28b655bafc98eab9543fe099229910f302d10`. The [report](evidence/milestones/runtime-prism-2026-10-12/prism-linux-search-wave/boot-smoke.json) and [capture manifest](evidence/milestones/runtime-prism-2026-10-12/prism-linux-search-wave/capture-manifest.json) record this run. Later source changes need fresh captures before these pictures can represent them. They do not certify hardware display behavior, complex-script shaping, full accessibility services, or Next 1.0 completion.

Prism uses Microsoft Selawik, an open-source metric-compatible replacement for Segoe UI; it is not Segoe UI Variable. The raster atlas, provenance, and unsupported shaping limits are listed in the [font notices](third-party/README.md#font-assets).

![Overview contact sheet of all twenty guest captures](images/prism-overview.png)

Regenerate the source captures with:

```sh
source tools/env.sh
bash tools/build.sh
python3 tools/boot_smoke.py --cpu qemu64 --timeout 360 --capture-prism-gallery \
  --image build/next-dev.img --output-dir build/runtime-linux-search-timer-zero-qemu64-r10
```

The gallery includes six kernel-hosted first-party panels, Home and Settings search, saved Accessibility preferences, Settings-only 125% text size, the shortcut guide and Notifications. Settings has 12 categories: Appearance has active theme/accent controls and Save/Revert, Accessibility exposes two saved preferences, System offers read-only diagnostics, and nine categories remain informational with unavailable settings rows. Search result selection and shortcut-guide activation are checked through the guest's accessible tree. Text scaling applies only within Settings and does not establish screen reader, speech, magnification or other unavailable services. The four-way snap frame shows Terminal in the BottomLeft work-area quadrant with Editor behind it; snapping remains limited to the single current work area. Files captures cover root and nested tabs, the I-beam location cursor, independent panes and a verified cross-pane copy. The Linux `cwd.elf` fixture exits 56 after checking `getcwd`, `chdir` and relative `openat`; the Linux timefile fixture exits 57 after checking `gettimeofday` sequential copyout and `ftruncate` descriptor semantics. The Calculator is an expression utility, not a scientific calculator. These captures and checks do not imply that every Settings category or planned Prism feature is complete.

## Capture index

Each thumbnail links to its full-size QEMU capture.

<table>
  <tr>
    <td align="center"><a href="images/prism-home.png"><img src="images/prism-home.png" alt="Prism Home with pinned applications and taskbar" width="180"></a><br><sub>Home</sub></td>
    <td align="center"><a href="images/prism-home-search-terminal.png"><img src="images/prism-home-search-terminal.png" alt="Home search with Next Terminal selected" width="180"></a><br><sub>Home search</sub></td>
    <td align="center"><a href="images/prism-terminal.png"><img src="images/prism-terminal.png" alt="Prism Terminal" width="180"></a><br><sub>Terminal</sub></td>
    <td align="center"><a href="images/prism-files.png"><img src="images/prism-files.png" alt="Prism Files browsing the native store" width="180"></a><br><sub>Files</sub></td>
  </tr>
  <tr>
    <td align="center"><a href="images/prism-files-tabs-root.png"><img src="images/prism-files-tabs-root.png" alt="Files root tab active beside a nested tab" width="180"></a><br><sub>Files root tab</sub></td>
    <td align="center"><a href="images/prism-files-tabs-nested.png"><img src="images/prism-files-tabs-nested.png" alt="Files nested tab active with the root retained" width="180"></a><br><sub>Files nested tab</sub></td>
    <td align="center"><a href="images/prism-files-tabs-ibeam.png"><img src="images/prism-files-tabs-ibeam.png" alt="I-beam cursor over the Files location field" width="180"></a><br><sub>Files location cursor</sub></td>
    <td align="center"><a href="images/prism-files-dual-pane.png"><img src="images/prism-files-dual-pane.png" alt="Files panes with independent paths" width="180"></a><br><sub>Files dual pane</sub></td>
  </tr>
  <tr>
    <td align="center"><a href="images/prism-files-copy-to-other-pane.png"><img src="images/prism-files-copy-to-other-pane.png" alt="Files copying a selected file into the other pane" width="180"></a><br><sub>Files cross-pane copy</sub></td>
    <td align="center"><a href="images/prism-settings.png"><img src="images/prism-settings.png" alt="Settings Appearance page" width="180"></a><br><sub>Settings Appearance</sub></td>
    <td align="center"><a href="images/prism-settings-search-keyboard.png"><img src="images/prism-settings-search-keyboard.png" alt="Settings search filtered for keyboard preferences" width="180"></a><br><sub>Settings search</sub></td>
    <td align="center"><a href="images/prism-settings-accessibility.png"><img src="images/prism-settings-accessibility.png" alt="Settings with saved Strong focus outline" width="180"></a><br><sub>Accessibility preferences</sub></td>
  </tr>
  <tr>
    <td align="center"><a href="images/prism-settings-text-scale.png"><img src="images/prism-settings-text-scale.png" alt="Settings with saved 125 percent text size" width="180"></a><br><sub>Settings text size</sub></td>
    <td align="center"><a href="images/prism-monitor.png"><img src="images/prism-monitor.png" alt="Prism System Monitor" width="180"></a><br><sub>System Monitor</sub></td>
    <td align="center"><a href="images/prism-editor.png"><img src="images/prism-editor.png" alt="Prism Editor" width="180"></a><br><sub>Editor</sub></td>
    <td align="center"><a href="images/prism-calculator.png"><img src="images/prism-calculator.png" alt="Prism Calculator expression view" width="180"></a><br><sub>Calculator</sub></td>
  </tr>
  <tr>
    <td align="center"><a href="images/prism-snap-quadrant.png"><img src="images/prism-snap-quadrant.png" alt="Terminal snapped to the lower-left quadrant" width="180"></a><br><sub>Quarter snap</sub></td>
    <td align="center"><a href="images/prism-shortcuts.png"><img src="images/prism-shortcuts.png" alt="Prism keyboard shortcut guide" width="180"></a><br><sub>Keyboard shortcuts</sub></td>
    <td align="center"><a href="images/prism-notifications.png"><img src="images/prism-notifications.png" alt="Prism notification center" width="180"></a><br><sub>Notifications</sub></td>
    <td align="center"><a href="images/prism-home-search-calculation.png"><img src="images/prism-home-search-calculation.png" alt="Home search with the arithmetic result 2+3*4 = 14 selected" width="180"></a><br><sub>Home arithmetic search</sub></td>
  </tr>
</table>

## Home

![Prism Home with centered pinned apps and taskbar](images/prism-home.png)

## Home search

![Prism Home with offline local search selecting Next Terminal](images/prism-home-search-terminal.png)

## Home search arithmetic

![Prism Home search with the evaluated arithmetic result 2+3*4 = 14 selected](images/prism-home-search-calculation.png)

Home search exposes this valid expression as a selectable result; Enter opens Calculator with the expression evaluated. The guest smoke checks the selected accessibility result and launch.

## Terminal

![Next Terminal in the Prism desktop](images/prism-terminal.png)

## Files

![Prism Files panel](images/prism-files.png)

The following frames document separate Files tab views and cursor state.

### Files tabs: root view

![Prism Files with the /native root tab active while a second tab retains /native/alpha](images/prism-files-tabs-root.png)

### Files tabs: nested view

![Prism Files with /native/alpha active while the root tab remains open](images/prism-files-tabs-nested.png)

### Files location I-beam cursor

![Prism Files showing the I-beam pointer over its editable location field](images/prism-files-tabs-ibeam.png)

### Independent panes and cross-pane copy

![Prism Files with independent left and right pane paths](images/prism-files-dual-pane.png)

![Prism Files showing a verified file copy into the other pane](images/prism-files-copy-to-other-pane.png)

The guest flow checks exact source and destination paths and confirms that both pane listings refresh after the commit.

## System Monitor

![Prism System Monitor panel](images/prism-monitor.png)

## Settings

![Prism Settings Appearance page with active theme and accent controls](images/prism-settings.png)

## Settings search

![Settings search for keyboard preferences, with Accessibility selected](images/prism-settings-search-keyboard.png)

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
