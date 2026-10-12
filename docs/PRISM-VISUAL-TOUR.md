# Prism visual tour

These twenty current captures come from the signed qemu64 guest image, SHA-256
`6b245fa85c2c49ad1da63cf7d6fdfb813e3d351c3275217c736d7b1c43b1da15`. Its QEMU
smoke passed 138 checks. The [report](evidence/milestones/runtime-prism-2026-10-12/prism-clock-notifications-wave/boot-smoke.json)
and [capture manifest](evidence/milestones/runtime-prism-2026-10-12/prism-clock-notifications-wave/capture-manifest.json)
record the run. It checked Home and Settings search through the guest's
accessible tree, saved Settings states, Files tab and pane navigation,
cross-pane copy, Home arithmetic selection of `2+3*4 = 14`, and Linux
`clock_gettime`/`clock_getres` fixture runs. These are development-build
captures tied to that image. They do not imply Next 1.0 completion, hardware
validation, full accessibility services, or that every Settings category is
functional.

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

## Home search arithmetic

![Prism Home search with the evaluated arithmetic result 2+3*4 = 14 selected](images/prism-home-search-calculation.png)

The guest selects the accessible `2+3*4 = 14` result. Enter opens Calculator with the expression evaluated; the gallery run checks the result selection and launch.

## Architecture schematics

These compact diagrams summarize the current implementation paths behind the
captured screens. They describe the bounded kernel-hosted development UI, not
the planned Next 1.0 compositor or service architecture.

### Desktop shell and panels

```mermaid
flowchart LR
    Input[Keyboard and pointer input] --> Shell[Prism shell]
    Shell --> Window[Window, focus, and snap state]
    Window --> Home[Home]
    Window --> Files[Files]
    Window --> Terminal[Terminal]
    Window --> Monitor[System Monitor]
    Window --> Settings[Settings]
    Window --> Editor[Editor]
    Window --> Calculator[Calculator]
    Window --> Overlays[Shortcuts and notifications]
    Home --> Renderer[Software framebuffer renderer]
    Files --> Renderer
    Terminal --> Renderer
    Monitor --> Renderer
    Settings --> Renderer
    Editor --> Renderer
    Calculator --> Renderer
    Overlays --> Renderer
```

QEMU captures show these panels rendered in the guest; they do not show
separate userspace applications or a GPU compositor.

### Files panes and tab contexts

```mermaid
flowchart LR
    Files[Files workspace] --> Left[Pane 1]
    Files --> Right[Pane 2]
    Left --> TabsA[Up to four tab contexts]
    Right --> TabsB[Up to four tab contexts]
    TabsA --> StateA[Independent path and listing state]
    TabsB --> StateB[Independent path and listing state]
```

The QEMU flow creates a nested folder, switches between root and nested tabs,
checks the I-beam pixels, and navigates each pane while retaining the other
pane's path. The four-tab bound comes from the implementation model; this smoke
does not fill every tab slot.

The Copy to other pane action uses the existing authorized Inspect/Commit flow.
The guest report records the exact source/target pair and confirms that both
pane listings refresh after the commit.

### Settings preference path

```mermaid
flowchart LR
    Controls[Appearance and Accessibility controls] --> Model[Settings preference model]
    Model --> Save[Save or Revert]
    Save --> Record[Checksummed appearance preference record]
    Record --> View[Updated Settings view]
```

The guest smoke confirms an accepted Settings Save action and captures the
Strong focus outline and 125% text-size states. Text scaling applies inside
Settings only. Host save/reload tests cover the preference record; these guest
screens do not establish screen-reader, magnification, speech, or other
assistive services.
