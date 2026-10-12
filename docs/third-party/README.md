# Third-party Rust dependencies

This inventory records the exact Cargo dependency graph, registry archive
SHA-256 checksums, resolved features, and original license notices. It includes
host build tools and conditional packages; inclusion does not imply that every
component is linked into the kernel or EFI binary. These are libraries and
compiler tools, not a foreign operating-system kernel or hypervisor.

The signed-package verifier uses `sha2` 0.10.9 and `ed25519-dalek` 2.2.0 with
default features disabled. The project does not claim a separate security audit
of these versions. Their upstream license/copyright notices are preserved here.
Toolchains, QEMU, OVMF, and host utilities are outside this Cargo-only inventory.

Regenerate offline after dependency changes:

```
source tools/env.sh
cargo metadata --locked --offline --format-version 1 > work/metadata.json
python3 tools/third_party.py
```

The [CycloneDX SBOM](sbom.cdx.json) records the full graph and archive hashes.
The [license manifest](license-manifest.json) hashes the copied notice files.

| Crate | Version | SPDX license expression | Original notices |
| --- | --- | --- | --- |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/block-buffer-0.10.4/LICENSE-APACHE), [LICENSE-MIT](licenses/block-buffer-0.10.4/LICENSE-MIT) |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/cfg-if-1.0.5/LICENSE-APACHE), [LICENSE-MIT](licenses/cfg-if-1.0.5/LICENSE-MIT) |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/cpufeatures-0.2.17/LICENSE-APACHE), [LICENSE-MIT](licenses/cpufeatures-0.2.17/LICENSE-MIT) |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/crypto-common-0.1.7/LICENSE-APACHE), [LICENSE-MIT](licenses/crypto-common-0.1.7/LICENSE-MIT) |
| curve25519-dalek | 4.1.3 | BSD-3-Clause | [LICENSE](licenses/curve25519-dalek-4.1.3/LICENSE) |
| curve25519-dalek-derive | 0.1.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/curve25519-dalek-derive-0.1.1/LICENSE-APACHE), [LICENSE-MIT](licenses/curve25519-dalek-derive-0.1.1/LICENSE-MIT) |
| digest | 0.10.7 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/digest-0.10.7/LICENSE-APACHE), [LICENSE-MIT](licenses/digest-0.10.7/LICENSE-MIT) |
| ed25519 | 2.2.3 | Apache-2.0 OR MIT | [LICENSE-APACHE](licenses/ed25519-2.2.3/LICENSE-APACHE), [LICENSE-MIT](licenses/ed25519-2.2.3/LICENSE-MIT) |
| ed25519-dalek | 2.2.0 | BSD-3-Clause | [LICENSE](licenses/ed25519-dalek-2.2.0/LICENSE) |
| fiat-crypto | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause | [COPYRIGHT](licenses/fiat-crypto-0.2.9/COPYRIGHT), [LICENSE-APACHE](licenses/fiat-crypto-0.2.9/LICENSE-APACHE), [LICENSE-BSD-1](licenses/fiat-crypto-0.2.9/LICENSE-BSD-1), [LICENSE-MIT](licenses/fiat-crypto-0.2.9/LICENSE-MIT) |
| generic-array | 0.14.7 | MIT | [LICENSE](licenses/generic-array-0.14.7/LICENSE) |
| libc | 0.2.190 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/libc-0.2.190/LICENSE-APACHE), [LICENSE-MIT](licenses/libc-0.2.190/LICENSE-MIT) |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/proc-macro2-1.0.107/LICENSE-APACHE), [LICENSE-MIT](licenses/proc-macro2-1.0.107/LICENSE-MIT) |
| quote | 1.0.47 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/quote-1.0.47/LICENSE-APACHE), [LICENSE-MIT](licenses/quote-1.0.47/LICENSE-MIT) |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/rustc_version-0.4.1/LICENSE-APACHE), [LICENSE-MIT](licenses/rustc_version-0.4.1/LICENSE-MIT) |
| semver | 1.0.28 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/semver-1.0.28/LICENSE-APACHE), [LICENSE-MIT](licenses/semver-1.0.28/LICENSE-MIT) |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/sha2-0.10.9/LICENSE-APACHE), [LICENSE-MIT](licenses/sha2-0.10.9/LICENSE-MIT) |
| signature | 2.2.0 | Apache-2.0 OR MIT | [LICENSE-APACHE](licenses/signature-2.2.0/LICENSE-APACHE), [LICENSE-MIT](licenses/signature-2.2.0/LICENSE-MIT) |
| subtle | 2.6.1 | BSD-3-Clause | [LICENSE](licenses/subtle-2.6.1/LICENSE) |
| syn | 2.0.119 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/syn-2.0.119/LICENSE-APACHE), [LICENSE-MIT](licenses/syn-2.0.119/LICENSE-MIT) |
| typenum | 1.20.1 | MIT OR Apache-2.0 | [LICENSE](licenses/typenum-1.20.1/LICENSE), [LICENSE-APACHE](licenses/typenum-1.20.1/LICENSE-APACHE), [LICENSE-MIT](licenses/typenum-1.20.1/LICENSE-MIT) |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 | [LICENSE-APACHE](licenses/unicode-ident-1.0.26/LICENSE-APACHE), [LICENSE-MIT](licenses/unicode-ident-1.0.26/LICENSE-MIT), [LICENSE-UNICODE](licenses/unicode-ident-1.0.26/LICENSE-UNICODE) |
| version_check | 0.9.5 | MIT OR Apache-2.0 | [LICENSE-APACHE](licenses/version_check-0.9.5/LICENSE-APACHE), [LICENSE-MIT](licenses/version_check-0.9.5/LICENSE-MIT) |

## Font assets

Prism's new source includes Open Sans Regular 1.10 from Debian fonts-open-sans 1.11-2. This font is Apache-2.0 licensed, not OFL. The unchanged source TTF, Google 2010–2011 copyright, original Debian attribution, Apache license and transformation NOTICE are preserved. The alpha atlas and advance tables were rasterized at 14/16/20 pixels with Pillow 12.3.0 / FreeType 2.14.3. The [asset SBOM](assets.cdx.json) and [font provenance](font-provenance.json) identify exact source/output hashes separately from the Cargo dependency SBOM. The retained [font generator](../../tools/prism_font_atlas.py) pins the source TTF digest, 24×32 cells, glyph codes 32–126, draw origin (0, −1), and rounded advances. Replaying it with Pillow 12.3.0 / FreeType 2.14.3 reproduced both checked-in arrays byte-for-byte. The generator checks the font digest; it does not enforce rasterizer versions. Only the raster arrays are embedded in the kernel; the unchanged TTF is retained as public source provenance.

Original notices: [copyright](assets/open-sans/debian-copyright.txt), [license](assets/open-sans/Apache-2.0.txt), [NOTICE](assets/open-sans/NOTICE.txt). Font/theme source additions are outside the immutable seventh image; guest visual validation must be recorded against a later image. Research-only external design references are not distributed assets.

Prism’s 640×400 RGB wallpaper is original procedural artwork under the project’s Apache-2.0 license. Its [retained generator](../../tools/prism_wallpaper.py), [provenance record](wallpaper-provenance.json), and asset SBOM record its exact bytes. Scratch replay reproduced all 768,000 bytes; no external wallpaper or research-reference pixels are embedded. This source replay is separate from future guest visual acceptance.


Prism uses Selawik Regular, Microsoft’s SIL Open Font License 1.1 metric-compatible open-source replacement for Segoe UI. It is not Segoe UI Variable. The checked-in upstream UFO source archive, built TTF, OFL notice, raster tables, pinned build requirements, and reproducible generator are identified by exact hashes in [Selawik font provenance](selawik-font-provenance.json). Microsoft notes that Selawik lacks Segoe UI kerning and needs improved hinting. The 24×32 atlas covers printable ASCII, printable Latin-1 (U+00A0–U+00FF), and 23 punctuation codepoints used by the UI and European text at 14/16/20 px; signed offsets preserve negative side bearings. A sorted little-endian table maps the added codepoints without allocation. The pinned font lacks U+00AA and U+00BA, which use a visible `?`; U+00A0 aliases space, and unavailable typographic dash/quote forms alias their ASCII equivalents. Other unsupported characters also visibly fall back to `?`. There is no shaping or grapheme composition.
