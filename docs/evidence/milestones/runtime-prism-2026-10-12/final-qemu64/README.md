# Final qemu64 guest run and Prism gallery

The signed development image SHA-256 is `b79790f446b5bde892fbde78d63e3a177d558d2d1319556680d9f48f24efb8ce`. The q35/TCG qemu64 run passed 119 checks and captured ten completed 1280 x 800 guest frames: Home, six first-party panels, Accessibility Settings after saving the Strong focus outline, keyboard shortcuts, and notifications. The accepted Settings save is visible in the report and serial log. `prism-settings-accessibility.png` shows the selected saved setting and the rendered focus ring.

The smoke harness checks guest frame dimensions/nonblank output and real QMP keyboard navigation; it does not certify visual quality, assistive technology, physical display behavior, or Next 1.0 completion.
