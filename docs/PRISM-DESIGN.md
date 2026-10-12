# Prism visual direction

The development desktop uses light cool surfaces, restrained blue accents,
rounded windows, an original folded blue wallpaper, and a centered application
bar. The Selawik Regular raster atlas is integrated at 14/16/20 px with signed
glyph bearings preserved.
Selawik is Microsoft’s open-source, metrically compatible Segoe UI replacement,
not Segoe UI Variable itself; its OFL license, upstream source, deterministic
build, and raster provenance are recorded under docs/third-party. Microsoft
Windows 11 documentation informed spacing, corner treatment, and hierarchy.
The wallpaper and icons are original Next artwork; reference pixels are not
embedded in the operating system. Prism has original, allocation-free arrow
and centered I-beam raster shapes with contrasting outlines and edge clipping;
it does not bundle Microsoft's cursor artwork. The Shell routes editable Files
fields to the I-beam and other desktop regions to the arrow. The current guest
gallery shows both cursor routes and independent Files root/nested tabs. Captures
show rendering, but do not replace font-layout, scaling, accessibility, or
physical-display validation.

![Prism Home from a completed guest framebuffer capture](images/prism-home.png)

See the [guest-captured visual gallery](PRISM-VISUAL-GALLERY.md) for Home,
Terminal, Files, Monitor, Settings, Editor, Calculator, notifications, the
keyboard shortcut guide, saved Accessibility settings, quarter snapping, Files
root/nested tabs, and the location-field I-beam. Settings exposes
12 navigable categories: Appearance has active theme/accent controls with
Save/Revert, Accessibility persists a Strong keyboard-focus outline and 100%
or 125% text size within Settings, System provides read-only diagnostics, and
the other nine pages are informational with disabled rows. The guest gallery
includes a frame after saving the 125% choice; the preference is scoped to
Settings text, not desktop-wide scaling.

## Reviewed reference images

- [geometry_hero_1880.png](https://raw.githubusercontent.com/MicrosoftDocs/windows-dev-docs/docs/hub/apps/design/signature-experiences/images/geometry_hero_1880.png)
- [geometry_rounded_corners_1880.png](https://raw.githubusercontent.com/MicrosoftDocs/windows-dev-docs/docs/hub/apps/design/signature-experiences/images/geometry_rounded_corners_1880.png)
- [materials_mica_hero_1880.png](https://raw.githubusercontent.com/MicrosoftDocs/windows-dev-docs/docs/hub/apps/design/signature-experiences/images/materials_mica_hero_1880.png)
- [color_light_controls_940.png](https://raw.githubusercontent.com/MicrosoftDocs/windows-dev-docs/docs/hub/apps/design/signature-experiences/images/color_light_controls_940.png)

## Current implementation limits

This is the bounded kernel-hosted development shell, with software rendering.
It is not the isolated production compositor. Selawik now covers printable
ASCII, printable Latin-1, and a fixed set of 23 typographic punctuation marks;
unsupported codepoints safely show `?`. It does not perform Unicode shaping,
combining-mark composition, bidirectional layout, or full Unicode display, and
UTF-8 editor storage does not imply those capabilities.
Bounded persisted appearance themes, real GUI file operations, the focus
outline, and Settings-only text scaling have dedicated tests and guest-frame
evidence. Desktop-wide wallpaper, font, density, text scaling and layout controls
remain unavailable; System is read-only and nine categories are informational.
Screen-reader transport, captions, speech, magnification, reduced motion,
keyboard-repeat and pointer preferences remain unavailable. Prism snapping now
supports left/right halves and four corner quadrants in one work area, with
reversible previews, minimum-size checks, and restore bounds; it does not provide
multi-monitor layout negotiation. Trash, client input, cooperative close, and
session-only Undo have separate source and guest gates; controls must preserve
backend authority and trusted-dialog priority. See
[PRISM-ACCEPTANCE.md](PRISM-ACCEPTANCE.md) for functional gates.

Licenses, exact source-font hashes, rasterization parameters, and reproducible
asset generators are recorded under docs/third-party and crates/prism/assets.
