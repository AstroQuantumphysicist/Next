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
embedded in the operating system. Prism now draws an original high-contrast
Windows-style pointer shape with a dark contour and edge clipping; it does not
bundle Microsoft's cursor artwork. The current font and cursor have host raster
tests, while their appearance in a frozen guest image remains pending.

## Reviewed reference images

- [geometry_hero_1880.png](https://raw.githubusercontent.com/MicrosoftDocs/windows-dev-docs/docs/hub/apps/design/signature-experiences/images/geometry_hero_1880.png)
- [geometry_rounded_corners_1880.png](https://raw.githubusercontent.com/MicrosoftDocs/windows-dev-docs/docs/hub/apps/design/signature-experiences/images/geometry_rounded_corners_1880.png)
- [materials_mica_hero_1880.png](https://raw.githubusercontent.com/MicrosoftDocs/windows-dev-docs/docs/hub/apps/design/signature-experiences/images/materials_mica_hero_1880.png)
- [color_light_controls_940.png](https://raw.githubusercontent.com/MicrosoftDocs/windows-dev-docs/docs/hub/apps/design/signature-experiences/images/color_light_controls_940.png)

## Current implementation limits

This is the bounded kernel-hosted development shell, with software rendering.
It is not the isolated production compositor. The Latin atlas does not provide
Unicode shaping; UTF-8 editor storage does not imply complete Unicode display.
Bounded persisted appearance themes and real GUI file operations have dedicated
retained-image guest evidence. Newer Trash, client input, cooperative close, and
session-only Undo have separate source and guest gates; controls must preserve
backend authority and trusted-dialog priority. Text size remains fixed at 100%. See [PRISM-ACCEPTANCE.md](PRISM-ACCEPTANCE.md) for functional gates.

Licenses, exact source-font hashes, rasterization parameters, and reproducible
asset generators are recorded under docs/third-party and crates/prism/assets.
