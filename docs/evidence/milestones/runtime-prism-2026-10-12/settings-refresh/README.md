# Prism Settings refresh — 2026-10-12

This is the qemu64 guest capture from the refreshed Settings UI. The matching
QEMU report records image SHA-256
`e69aa9ca8de049c2a12d048e28e56525a0c352992b2385b200daba70de4ab484` and 118
passed checks. The 1280×800 Settings frame was taken after a completed software
framebuffer presentation through the Prism gallery's QMP-driven run. It shows
all 12 categories; only Appearance has active settings controls, System offers
read-only diagnostics, and the other ten categories are informational with
disabled rows. The capture is a visual/rendering artifact, not proof of hardware
behavior, screen-reader support, or controls hidden behind disabled rows.

`prism-settings.png` is the exact guest Settings frame published in
`docs/images/prism-settings.png`. The raw PPM guest capture was
`build/prism-gallery/settings.ppm` during the run.
