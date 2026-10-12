# Prism acceptance and next implementation gates

The [current guest visual gallery](PRISM-VISUAL-GALLERY.md) shows the six
development panels, Home, notifications and the shortcut guide. It is a visual
record of the current working tree; the source-pinned gates below remain the
acceptance evidence for their named images and do not inherit this gallery's
results.

This review compares the eighth development desktop source and the actual 1280×800 QEMU screenshot at `build/eighth-qemu64/screenshot.ppm` with the unchanged Next 1.0 spec §§13–14 and 22–23. The eighth image is `e1aa0516b94e20f349cea2bbe4593bbce4d27e3ac4b014e7971fe635b267741e`; its frozen 207-file source manifest is `work/eighth-wave/source-manifest.json`, SHA-256 `37b72e7d411ae6f37bbd845fbe8c3c9d4c8dda6db6ebfde96fe4566fd3bc9ee1`. This is a scoped review, not a complete-product acceptance result. The requirement ledger remains tied to its last completed immutable milestone until the eighth proof set is reconciled.

The inspected screenshot has legible antialiased text, consistent light surfaces, rounded window/taskbar chrome, recognizable original icons and a coherent blue procedural wallpaper. The terminal text and controls fit within the visible window, and the taskbar stays separate from it. This screenshot shows only Terminal; it cannot establish Files, Home, notifications, Settings, modal dialogs, scaling, keyboard access or interaction correctness. A reviewer’s preference for the design does not substitute for those functional checks.

The existing `boot_smoke.py` screenshot assertion checks dimensions and at least four distinct colors. Its own report correctly states that this is not visual layout or accessibility verification. Frame generations and command responses prove completed software presentations, not text shaping, compositor isolation, scanout timing or polished behavior across all screens.

| Area | Eighth-image bounded implementation | Required next acceptance gate at that review |
|---|---|---|
| Visual system | Selawik Regular ASCII raster atlas staged for integration at 14/16/20 px (metric-compatible open-source Segoe UI replacement, not Segoe UI Variable), original wallpaper, one light palette, software framebuffer | Inspect each real app at 1280×800 and smaller supported viewport; long titles and errors must remain readable without overlapping controls. Add dark/high-contrast palettes and explicit text-size settings before claiming theme/scaling coverage. |
| Window management | Five built-in panels, focus/order, minimize/maximize/restore, pointer drag/resize, snap previews and minimum bounds | QMP keyboard and pointer actions must produce matching window state and completed frames; cancel must restore prior bounds. Multiple instances, workspaces and client-owned surfaces remain separate missing features. |
| Settings | At the eighth-image review, an explicitly read-only diagnostic panel. The current tree has 12 navigable categories: Appearance has active theme/accent selection and Save/Revert through the existing persistence adapter; System exposes read-only diagnostics; Display, Sound, Network & Internet, Bluetooth & Devices, Apps, Accounts, Privacy & Security, Storage & Backup, Accessibility, and Developer are informational pages with disabled setting rows. | Complete §22.3 by implementing and validating the remaining appearance controls and real controls/persistence for other categories. Wallpaper, fonts, density, scale, desktop layout, hardware/system controls outside the existing diagnostics, and full assistive-technology coverage remain unavailable or unvalidated; the latest qemu64 frame shows rendering only. |
| Files | Current authoritative file bindings, selection/open, trusted file portal and Editor backend operations | Expose real create/rename/copy/delete through GUI controls; verify actual stored bytes and directory refresh. Cancellation and denied/failed writes must preserve the old file. Permanent delete needs explicit impact confirmation; do not label deletion as Trash until restoration exists. |
| Home/Search | Offline built-in app/file search and exact actions | Verify keyboard-only result selection, no-result/cancel behavior, removed-file refresh and focus restoration. Installed-package catalog, favorites/order persistence and authorized content indexing remain absent. |
| Notifications | Eight session-only trusted-adapter records, origin, priority, read/dismiss, DND and keyboard controls | Generate a real process/service event, verify origin and unread count, dismiss by both input paths, and prove dismissal does not terminate a process or alter a file. Verify DND suppresses alerts while retaining history. Persistent publisher permission, quiet hours, sound and lock-screen privacy remain absent. |
| Typography | Source strings can contain UTF-8; atlas renders unsupported characters as `?` | Clearly describe ASCII display coverage. Add fallback/shaping and grapheme-safe navigation before claiming Unicode/RTL/IME accessibility. Test filename display with multibyte text; preserving backend bytes alone does not establish readable text. |
| Accessibility | Some keyboard interactions and visible focus states | Audit every actionable control for a keyboard path, visible selection, accessible name/role/state and disabled/error behavior. Screen-reader tree/events, magnifier, high contrast and secure accessible prompts remain unimplemented. |
| Terminal/Monitor | Bounded command console, supplied truthful counters, process output queue | Preserve prompt/output under resize and load; distinguish unavailable counters. PTY, job control, ANSI terminal compatibility, selection/clipboard and arbitrary shell execution remain absent. |
| Trust boundaries | Kernel-hosted panels and real rights-limited file selection | User compositor/native surface protocol, supervised shell restart, untrusted app GUI isolation and cross-runtime clipboard/drag offers are independent missing architecture gates. |

At the eighth-image review, the strongest implementation priorities were a real persistent Appearance page and GUI file operations over the existing authoritative store. They turn visible shell controls into useful daily actions without implying a full compositor, general filesystem or installed-app ecosystem. A bounded preferences object can be independently tested for versioning, invalid values and failure atomicity before integration. The existing store’s file/path/capacity limits must remain explicit.

For each new UI feature, retain a screenshot of its normal and error states, raw input/action timestamps, completed-frame generations, authoritative backend outcome and image/source identities. Use actual QMP keyboard and pointer events rather than invoking shell state directly. A screenshot pixel count is a supporting sanity check. Persistence, permissions and irreversible-action behavior require independent state checks. Reference TCG latency measurements remain development references, not hardware, input-to-photon or accessibility certification.

## Ninth source preview review (not eighth guest proof)

Independent inspection of `work/prism-nine-settings-{light,dark,contrast}.ppm` and `work/prism-nine-delete.ppm` found readable normal and dark text, a bounded Appearance layout, explicit pending-save wording, and a permanent-delete dialog whose default is Cancel and whose text clearly says there is no Trash. These are host renders of newer source, not execution or persistence evidence for the immutable eighth image.

The High Contrast preview exposed a functional visual gap: surface, background and selected fill all become black. Theme and accent selection lose their visible selected state; identical row-wide underlines show the focus group but do not identify the checked option. A distinct checked marker or accent outline is required, and action buttons need visible boundaries. The owning UI agent has been asked to correct these states and test their actual rendered representation.

Independent sRGB contrast calculations put the existing light muted text `#64748B` at 4.512:1 on the app surface `#F7F9FC`, but 4.151:1 on background `#EAF0F8`. Small muted labels should remain on their tested app surface or use the primary text color. New source palette error/warning mappings meet 4.5:1 on Dark/High Contrast app/background surfaces. Existing light `#DC2626` on the background is 4.213:1 and remains an explicitly uncorrected legacy pairing. Theme-token unit tests do not establish all rendered controls or full accessibility conformance.

Font provenance remains narrow and truthful: the source TTF is Apache-2.0 and preserved unchanged; the atlas retains only glyphs U+0020–U+007E at three sizes. Scratch regeneration reproduced the raster and advance arrays exactly with the recorded Pillow/FreeType versions. This is reproducible asset transformation for those inputs, not full Unicode coverage, shaping, accessibility or guest rendering certification. Research screenshots are reference material, not copied or embedded image assets.

### Source 10 keyboard and Copy implementation

Bare Super toggles Home on release. Super+Space opens Search; Super+I, Super+E,
and Super+Enter open Settings, Files, and Terminal. Super+arrows retain the snap
preview flow. Alt+F4 closes the focused window; a dirty Editor requires Cancel,
Discard, or a successful actual save first. Trusted dialogs consume these keys,
and Super chords do not insert text into the current app. This implements these
shortcuts only; secure lock, workspaces, screen capture, and assistive technology
integration remain separate acceptance work.

Files adds a Copy button and C shortcut. The dialog inspects the actual destination,
defaults to Cancel before replacement, and emits a source/destination revision
conditional request. The backend copies under the journal lock and confirms its
commit before the UI selects an authoritative refreshed destination binding.
Failures retain the source and entered destination. Host interaction and conditional
backend tests cover this behavior; live image 10 proof is still required.

Source 10 also consumes two ordinary client pixel previews. Each keeps at most
64 KiB of copied pixel bytes in static Shell storage. Trusted producer metadata
supplies the complete signed package label and PID. Surface-object and frame
version checks reject stale publications and clear callbacks, including late
frames after destruction. RGB/BGR pixels are shown with bounded aspect-preserving
nearest-neighbor scaling. Publication never changes keyboard focus; preview clicks
are consumed rather than activating the trusted window underneath. Trusted prompts,
Home, notifications, and taskbar remain above the previews. This is a copied visual
consumer, not isolated compositor execution or client keyboard/pointer delivery.

### Source 11 trusted interaction and Trash

The working tree adds a bounded Trash page backed by authoritative items and
exact-generation requests. Files Delete now asks to move a native file to Trash.
Restore chooses a destination without silently replacing another file; permanent
removal has a separate Cancel-first confirmation. Failed operations retain the
selected item and input. Visible rows change only after the kernel supplies a
confirmed refreshed listing. The current development store supports two items and
one trusted session; multi-user retention and external-volume Trash are not claimed.
The backend's atomic/cache and corruption checks and the UI model are host-tested;
reboot and real guest Trash interactions still need their separate gate.

Client content gestures now ask the kernel broker for focus using the full surface
identity. Delivery begins only after an exact acknowledgment. The UI maps pointer
positions through the actual scaled content rectangle, forwards semantic presses,
and keeps caption/chrome separate from client pixels. A focus outline marks the
accepted client; revoked delivery shows a click-to-resume cue. Overflow and failed
focus preserve a blocked client context, preventing keystrokes or Alt+F4 from
reaching a trusted window behind it. Explicit trusted focus changes and exact
surface teardown end that context. Modal prompts suspend client delivery before
handling input. These host checks do not replace the pending actual client-input
fixture, hardware keyboard certification, modifier/release protocol, or isolated
user compositor execution.

### Source 12 cooperative native close

Native window chrome has an actual Close button, and Alt+F4 targets the current
client context. The kernel's authenticated negotiation/status decides whether the
UI may request close. Older unsupported clients display “Close unavailable”;
requests never close a trusted window underneath. The UI queues a full surface
identity and displays requested, closing, refused, failed, or unresponsive status
supplied by the broker. Requested, closing, and unresponsive tickets suppress
repeated gestures. Refusal and failed enqueue permit a later request. Application
responses remain advisory: pixels stay visible until real surface teardown.
Portal, Editor, Files, and Trash confirmations retain priority. No Force Quit or
process termination authority is provided by these controls. Host checks cover
negotiation, repeated gestures, advisory lifetime, stale identities, portal priority,
and caption bounds; protocol integration and real guest close evidence are separate
gates.

### Evidence updates and Source 12 Undo

The retained ninth corrected image `37da16e8388427af8182c4535f951f67e4879498e1b908d61499d6d29181cd79`
passed its actual two-boot Files/Appearance gate (119 create-boot and 117 verify-boot
checks). Real create, Editor save, rename replacement/cancellation, delete
confirmation/cancellation, protected-path failure, visible Shift+Tab, and persisted
32-byte appearance settings were checked against backend bytes and completed
frames. The historical eighth-image table above does not describe newer source.
Dark/high-contrast selection visibility was corrected before that guest gate.

The retained tenth image `60ab816c97cc2c510ac5eed76a3db2d039b924a72fe6904bc6eae63f36cbfc27`
passed the two-boot shortcuts/Copy gate (120 create-boot and 117 verify-boot checks).
Copy retained the source and changed actual destination bytes only after confirmed
replacement. Super shortcuts and dirty Editor Alt+F4 Cancel/Save/Discard used real
keyboard/pointer input and response-bound completed frames. Temporary Copy files
were removed before reboot, so this gate does not claim Copy persistence.

Source 12 adds session-only Undo for the latest eligible Move or Restore, using
an authoritative generation-bound receipt. U in Files/Trash and the Trash Undo
button emit a once-only typed action; stale or untaken callbacks fail. Failed Undo
retains the receipt and actual rows for retry; successful Undo consumes the receipt
and waits for an authoritative refreshed listing. Restore Undo re-trashes the
restored bytes; Move Undo restores to the original path without overwriting.
Intervening mutations, read-only storage, and reboot remove eligibility. Permanent
delete and overwrite have no Undo. The UI/model is host-tested; actual guest Undo
and Source 12 cooperative-close proof remain pending. Window switching now starts
at the correct recency edge when every open window is minimized, with both
forward and reverse host regressions.

### Source 13 semantic observation and Files queries

A bounded caller-owned accessibility snapshot now describes the actual active
trusted context, names, roles, values, enabled/visible/selected/checked/busy state,
and focus. Modal snapshots exclude background controls. Available geometry comes
from shared render/hit helpers; some existing controls still lack precise node
bounds. Context/focus/snapshot events deduplicate identical redraws; queue overflow
requires a fresh snapshot through Reset. The kernel observes this internally; no
app reader permission, screen-reader transport, speech, braille, native-pixel
content tree, foreign-runtime bridge, or accessibility certification is implied.
The keyboard models retain their actual current behavior; complete uniform Tab
navigation, enlarged text, pointer/caret aids and input preferences remain work.

Files has a real Filter dialog with a bounded ASCII query, Name/Size sort,
ascending/descending order, and explicit full-content search. Exact confirmed
backend paths determine a Files-only visible order; Home and the trusted file
portal retain the full authoritative bindings. Failed requests retain input and
prior order; stale/untaken callbacks cannot publish rows. Counts reflect the
backend's stored-file results, with the immutable boot document shown separately.
The backend searches full checksummed payloads, not GUI excerpts, and private Trash
keys remain excluded. Keyboard/pointer modal and scope-preservation host tests are
present; Source 13 actual guest query/semantic-observation proof is still pending.

### Retained Source 11 and 12 guest updates

The retained eleventh image `8236ffef6154ef708c37ebe1cf2ba0a750b2c1fc1c543f799fc01060d2a35bee`
passed 122 actual client-input checks and the two-boot Trash gate, 117 checks
per boot. Trusted full-identity gestures delivered keys to the selected native
client; overflow reset, portal suspension, invalid copyout, rights attenuation,
and final-handle teardown were exercised. Trash used actual move, collision
refusal, alternate restore, retained open-file writes, reboot listing and
Cancel-first permanent removal. These scoped gates do not establish all other
validation families on this image.

The retained twelfth image `1dbcad2f1dd45d74383f5120deb9d1530d9dc594443f98e6617f4cd69b921c83`
passed 121 cooperative-close guest checks. The first actual Close click was
refused and retained the window; the second kept its pixels visible during
a bounded save. Stored-byte readback preceded application Exit51, followed
by exact-generation surface cleanup. This does not prove unresponsive-client
behavior or Force Quit. Session-only Undo remains under its separate guest gate.

### Source 14 native window integration

Native windows are gaining trusted placement, unified stacking, header dragging,
minimize and taskbar restoration. Chrome focus uses a distinct trusted action
and grants no synthetic client pointer event. The kernel records full-identity
placement metadata only after a successful framebuffer presentation. Hidden
windows retain their owner and latest publication; minimization cannot claim
surface destruction, nor can a hidden publication claim newly displayed pixels.
The retained Source 14 guest gate passed 123 qemu64 checks including two
overlapping windows, actual title dragging, minimize/restore/focus order and
generation-safe close. The baseline Max-CPU boot also passed 118 checks, but
expanded native-window actions on Max remain unrun. Native resize/configure
integration and production compositor isolation remain separate work.

### Source 15 retained resize and runtime-budget results

The frozen Source 15 image `5500b615bd61d7b2275c049f04bff546da999607db63f3e361b421553e6d9b7f`
uses the 3,553-file source manifest `work/fifteenth-wave/source-manifest.json`
(`b2f44957600996f6954ddf729fe1ca1011083cdeb71fae9a0ab9107683489e21`). The full
source test/build script returned zero. Its actual qemu64 native-resize guest
gate passed 123 checks: trusted resize changed the signed client from 64×48 to
81×59 and then 73×53, each followed by the exact new configure/ack and copied
checkerboard frame; stale configuration and old-buffer publication were denied,
and the resize did not grant focus. Actual Home, Files and Settings captures,
guarded emergency-stack probes, calibrated LAPIC continuation and zero UART
drops are retained in `docs/evidence/milestones/5500b615bd61/`.

The separate interactive CPU-budget gate did not pass: after 300 seconds the
sole desktop-launched spin process had no throttle marker, although startup
workers in the same image completed measured throttle/refill cycles. The desktop
poll loop slept after each native quantum; Source 16 now makes that sleep
conditional on no native work. Quota remains 100 ms per 200 ms. A new frozen
image and actual guest rerun are required before accepting this fix. Source 15
does not establish CPU-budget guest acceptance, Max-CPU resize, or nested-folder
guest behavior.

### Source 18 Prism refresh and nested Files guest proof

The Source 18 development image is `a6afc2715e8e0775c53a41f429ba0c84b4626f53c5d4017625d87f0ba85bc0f6` (68,157,440 bytes). Its retained 3,592-file source manifest is `docs/evidence/milestones/a6afc2715e8e/source-manifest.json`, SHA-256 `3bee43295ce414960cac5ca41855966c57f7f79407e67a9d6adae1473fc07989`. The image was built from the Source 18 production tree; a later testing-only correction to the directory gate's transition-only focus wait is included in this manifest and did not change guest sources or image bytes. The full workspace test/build run passed for that production tree, followed by the focused directory harness self-test.

Prism refreshes Home, taskbar, Files, Terminal, and Settings with a consistent light desktop, centered launcher, blue accent, updated app controls, original wallpaper, Selawik Regular raster text, and the original high-contrast pointer. Selawik is an open-source metric-compatible Segoe UI replacement, not Microsoft's Segoe UI font. Actual 1280×800 guest captures of Home, Files, and Settings are retained with the native resize report. The qemu64 guest resize gate passed two real native-window resizes (81×59, then 73×53), exact configure acknowledgments and presented frames, guarded emergency-stack probes, and calibrated LAPIC continuation. This remains a software-compositor QEMU result, not hardware or accessibility certification.

The source-bound two-boot Files gate passed on the same image. Real QMP input created and navigated folders, created a nested child, renamed and moved an empty folder across parents, refused removal of a non-empty folder, then removed empty folders. A separately compiled read-only Store oracle checked actual journal state after each operation and after reboot. The root listing and final empty-folder screenshots, action timeline, serial logs, reports, audio capture, source manifest and hashes are retained beside this review. This validates the bounded directory feature; it does not establish a general host filesystem or multi-user volume service.

The Source 18 snapshot recorded here had 195 requirements: 84 pending, 111 partial, 0 verified, and `product_ready=false`. The [current requirement ledger](STATUS.md) reflects later work; the full specification remains incomplete.
