//! Original Prism surfaces and icons with a Microsoft Selawik raster atlas.
use crate::{Bounds, Canvas};

pub const BACKGROUND: u32 = 0xeaf0f8;
pub const SURFACE: u32 = 0xf7f9fc;
pub const SURFACE_RAISED: u32 = 0xffffff;
pub const BORDER: u32 = 0xcbd5e3;
pub const TEXT: u32 = 0x17283f;
pub const MUTED: u32 = 0x64748b;
pub const ACCENT: u32 = 0x2368df;
pub const ACCENT_SOFT: u32 = 0xe2edff;

#[derive(Clone, Copy)]
pub enum Font {
    Small,
    Body,
    Title,
}
impl Font {
    fn index(self) -> usize {
        match self {
            Self::Small => 0,
            Self::Body => 1,
            Self::Title => 2,
        }
    }
}
const ASCII_GLYPH_COUNT: usize = 95;
const EXTRA_GLYPH_COUNT: usize = 119;
const GLYPHS_PER_SIZE: usize = ASCII_GLYPH_COUNT + EXTRA_GLYPH_COUNT;
static ATLAS: &[u8; 493056] = include_bytes!("../assets/selawik-regular-atlas.bin");
static ADVANCE: &[u8; 642] = include_bytes!("../assets/selawik-regular-advance.bin");
static X_OFFSET: &[u8; 642] = include_bytes!("../assets/selawik-regular-x-offset.bin");
static EXTRA_CODEPOINTS_LE: &[u8; EXTRA_GLYPH_COUNT * 2] =
    include_bytes!("../assets/selawik-regular-codepoints-le.bin");
fn glyph_index(c: char) -> usize {
    if (' '..='~').contains(&c) {
        c as usize - 32
    } else {
        let codepoint = c as u32;
        let mut low = 0;
        let mut high = EXTRA_GLYPH_COUNT;
        while low < high {
            let middle = low + (high - low) / 2;
            let offset = middle * 2;
            let candidate =
                u16::from_le_bytes([EXTRA_CODEPOINTS_LE[offset], EXTRA_CODEPOINTS_LE[offset + 1]])
                    as u32;
            if candidate < codepoint {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let offset = low * 2;
        if low < EXTRA_GLYPH_COUNT
            && u16::from_le_bytes([EXTRA_CODEPOINTS_LE[offset], EXTRA_CODEPOINTS_LE[offset + 1]])
                as u32
                == codepoint
        {
            ASCII_GLYPH_COUNT + low
        } else {
            b'?' as usize - 32
        }
    }
}
pub fn text_width(s: &str, font: Font) -> i32 {
    text_width_scaled(s, font, crate::appearance::TextScale::Percent100)
}
pub fn text_width_scaled(s: &str, font: Font, scale: crate::appearance::TextScale) -> i32 {
    s.chars().fold(0i32, |sum, c| {
        let advance = ADVANCE[font.index() * GLYPHS_PER_SIZE + glyph_index(c)] as i32;
        let advance = if scale == crate::appearance::TextScale::Percent125 {
            advance.saturating_mul(5).saturating_add(3) / 4
        } else {
            advance
        };
        sum.saturating_add(advance)
    })
}
pub fn text(canvas: &mut Canvas<'_>, x: i32, y: i32, s: &str, color: u32, font: Font) {
    let color = canvas.font_color(color);
    let scaled = canvas.text_scale() == crate::appearance::TextScale::Percent125;
    let mut pen = x;
    for c in s.chars() {
        let index = font.index() * GLYPHS_PER_SIZE + glyph_index(c);
        if pen >= canvas.width() as i32 {
            break;
        }
        let output_rows = if scaled { 40 } else { 32 };
        let output_cols = if scaled { 30 } else { 24 };
        let x_offset = X_OFFSET[index] as i8 as i32;
        let x_offset = if scaled {
            x_offset.saturating_mul(5) / 4
        } else {
            x_offset
        };
        for row in 0..output_rows {
            for col in 0..output_cols {
                let a = if scaled {
                    scaled_alpha(index, row, col)
                } else {
                    ATLAS[index * 768 + row * 24 + col]
                };
                if a != 0 {
                    blend_pixel(
                        canvas,
                        pen.saturating_add(x_offset).saturating_add(col as i32),
                        y.saturating_add(row as i32),
                        color,
                        a,
                    );
                }
            }
        }
        let advance = ADVANCE[index] as i32;
        let advance = if scaled {
            advance.saturating_mul(5).saturating_add(3) / 4
        } else {
            advance
        };
        pen = pen.saturating_add(advance);
    }
}
fn scaled_alpha(index: usize, row: usize, col: usize) -> u8 {
    let source_y = row * 4;
    let y0 = source_y / 5;
    let y1 = (y0 + 1).min(31);
    let fy = (source_y % 5) as u32;
    let source_x = col * 4;
    let x0 = source_x / 5;
    let x1 = (x0 + 1).min(23);
    let fx = (source_x % 5) as u32;
    let atlas = index * 768;
    let top_left = ATLAS[atlas + y0 * 24 + x0] as u32;
    let top_right = ATLAS[atlas + y0 * 24 + x1] as u32;
    let bottom_left = ATLAS[atlas + y1 * 24 + x0] as u32;
    let bottom_right = ATLAS[atlas + y1 * 24 + x1] as u32;
    let top_weight = 5 - fy;
    let left_weight = 5 - fx;
    let alpha = top_left * left_weight * top_weight
        + top_right * fx * top_weight
        + bottom_left * left_weight * fy
        + bottom_right * fx * fy;
    ((alpha + 12) / 25) as u8
}
fn blend_pixel(canvas: &mut Canvas<'_>, x: i32, y: i32, color: u32, alpha: u8) {
    if x < 0 || y < 0 || x as usize >= canvas.width || y as usize >= canvas.height {
        return;
    }
    let raw = canvas.pixels[y as usize * canvas.stride + x as usize];
    let old = canvas.encode(raw);
    let a = alpha as u32;
    let inv = 255 - a;
    let mut mixed = 0;
    for shift in [0, 8, 16] {
        mixed |=
            ((((color >> shift) & 255) * a + ((old >> shift) & 255) * inv + 127) / 255) << shift;
    }
    canvas.pixel_literal(x, y, mixed);
}
pub fn rounded_rect(canvas: &mut Canvas<'_>, b: Bounds, radius: i32, color: u32) {
    if b.width <= 0 || b.height <= 0 {
        return;
    }
    let r = radius.clamp(0, 64).min(b.width / 2).min(b.height / 2);
    let first = b.y.max(0);
    let last = b.y.saturating_add(b.height).min(canvas.height() as i32);
    for y in first..last {
        let row = y.saturating_sub(b.y);
        let dy = if row < r {
            r - row - 1
        } else if row >= b.height - r {
            row - (b.height - r)
        } else {
            0
        };
        let mut inset = 0;
        if dy > 0 {
            while inset < r && (r - inset - 1) * (r - inset - 1) + dy * dy > r * r {
                inset += 1;
            }
        }
        canvas.rect(b.x.saturating_add(inset), y, b.width - 2 * inset, 1, color);
    }
}
pub fn outline(canvas: &mut Canvas<'_>, b: Bounds, radius: i32, color: u32) {
    rounded_rect(canvas, b, radius, color);
    rounded_rect(
        canvas,
        Bounds {
            x: b.x.saturating_add(1),
            y: b.y.saturating_add(1),
            width: b.width.saturating_sub(2),
            height: b.height.saturating_sub(2),
        },
        radius.saturating_sub(1),
        SURFACE,
    );
}
pub fn shadow(canvas: &mut Canvas<'_>, b: Bounds, radius: i32) {
    if b.width <= 0 || b.height <= 0 {
        return;
    }
    // Paint only the visible narrow halo. Preserve the wallpaper beneath it,
    // rather than introducing solid blue borders or repainting the full panel.
    for (spread, alpha) in [(10, 4), (6, 8), (3, 14)] {
        let outer = Bounds {
            x: b.x.saturating_sub(spread),
            y: b.y.saturating_add(3).saturating_sub(spread),
            width: b.width.saturating_add(2 * spread),
            height: b.height.saturating_add(2 * spread),
        };
        let r = (radius + spread)
            .clamp(0, 64)
            .min(outer.width / 2)
            .min(outer.height / 2);
        for y in outer.y.max(0)
            ..outer
                .y
                .saturating_add(outer.height)
                .min(canvas.height as i32)
        {
            let row = y.saturating_sub(outer.y);
            let dy = if row < r {
                r - row - 1
            } else if row >= outer.height - r {
                row - (outer.height - r)
            } else {
                0
            };
            let mut inset = 0;
            if dy > 0 {
                while inset < r && (r - inset - 1) * (r - inset - 1) + dy * dy > r * r {
                    inset += 1;
                }
            }
            let start = outer.x.saturating_add(inset).max(0);
            let end = outer
                .x
                .saturating_add(outer.width - inset)
                .min(canvas.width as i32);
            let spans = if y < b.y || y >= b.y.saturating_add(b.height) {
                [(start, end), (end, end)]
            } else {
                [
                    (start, end.min(b.x)),
                    (start.max(b.x.saturating_add(b.width)), end),
                ]
            };
            for (first, last) in spans {
                for x in first..last {
                    blend_pixel(canvas, x, y, 0x020c1e, alpha);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
pub enum Icon {
    Next,
    Terminal,
    Files,
    Monitor,
    Settings,
    Editor,
    Calculator,
    Search,
    Notices,
    Close,
    Minimize,
    Maximize,
}
fn line(canvas: &mut Canvas<'_>, mut x: i32, mut y: i32, x1: i32, y1: i32, color: u32) {
    let dx = (x1 - x).abs();
    let sx = if x < x1 { 1 } else { -1 };
    let dy = -(y1 - y).abs();
    let sy = if y < y1 { 1 } else { -1 };
    let mut e = dx + dy;
    loop {
        canvas.rect_literal(x, y, 2, 2, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * e;
        if e2 >= dy {
            e += dy;
            x += sx;
        }
        if e2 <= dx {
            e += dx;
            y += sy;
        }
    }
}
pub fn icon(canvas: &mut Canvas<'_>, kind: Icon, x: i32, y: i32, size: i32, color: u32) {
    if !(8..=128).contains(&size) {
        return;
    }
    let color = canvas.font_color(color);
    let p = |n: i32| n * size / 24;
    let mut l = |a: i32, b: i32, c: i32, d: i32| {
        line(
            canvas,
            x.saturating_add(p(a)),
            y.saturating_add(p(b)),
            x.saturating_add(p(c)),
            y.saturating_add(p(d)),
            color,
        )
    };
    match kind {
        Icon::Next => {
            l(3, 19, 3, 5);
            l(3, 5, 20, 19);
            l(20, 19, 20, 5);
            l(9, 5, 20, 5);
        }
        Icon::Terminal => {
            l(4, 7, 9, 12);
            l(9, 12, 4, 17);
            l(12, 17, 20, 17);
        }
        Icon::Files => {
            l(2, 7, 9, 7);
            l(9, 7, 12, 10);
            l(12, 10, 21, 10);
            l(21, 10, 21, 19);
            l(21, 19, 2, 19);
            l(2, 19, 2, 7);
        }
        Icon::Monitor => {
            l(2, 4, 21, 4);
            l(21, 4, 21, 16);
            l(21, 16, 2, 16);
            l(2, 16, 2, 4);
            l(12, 16, 12, 21);
            l(7, 21, 17, 21);
        }
        Icon::Settings => {
            for (a, b, c, d) in [
                (7, 3, 16, 3),
                (16, 3, 21, 10),
                (21, 10, 17, 20),
                (17, 20, 6, 20),
                (6, 20, 2, 10),
                (2, 10, 7, 3),
                (9, 9, 15, 9),
                (15, 9, 15, 15),
                (15, 15, 9, 15),
                (9, 15, 9, 9),
            ] {
                l(a, b, c, d);
            }
        }
        Icon::Editor => {
            l(5, 2, 16, 2);
            l(16, 2, 21, 7);
            l(21, 7, 21, 22);
            l(21, 22, 5, 22);
            l(5, 22, 5, 2);
            l(9, 10, 17, 10);
            l(9, 14, 17, 14);
            l(9, 18, 14, 18);
        }
        Icon::Calculator => {
            l(5, 2, 19, 2);
            l(19, 2, 19, 22);
            l(19, 22, 5, 22);
            l(5, 22, 5, 2);
            l(8, 6, 16, 6);
            l(8, 11, 9, 12);
            l(14, 11, 15, 12);
            l(8, 16, 9, 17);
            l(14, 16, 15, 17);
        }
        Icon::Search => {
            l(5, 3, 13, 3);
            l(13, 3, 17, 7);
            l(17, 7, 17, 13);
            l(17, 13, 13, 17);
            l(13, 17, 5, 17);
            l(5, 17, 2, 13);
            l(2, 13, 2, 7);
            l(2, 7, 5, 3);
            l(16, 16, 22, 22);
        }
        Icon::Notices => {
            l(5, 17, 5, 8);
            l(5, 8, 9, 4);
            l(9, 4, 15, 4);
            l(15, 4, 19, 8);
            l(19, 8, 19, 17);
            l(3, 17, 21, 17);
            l(10, 21, 14, 21);
        }
        Icon::Close => {
            l(5, 5, 19, 19);
            l(19, 5, 5, 19);
        }
        Icon::Minimize => l(5, 16, 19, 16),
        Icon::Maximize => {
            l(5, 5, 19, 5);
            l(19, 5, 19, 19);
            l(19, 19, 5, 19);
            l(5, 19, 5, 5);
        }
    }
}
/// Original offline-generated ribbons. Rows are reused when scaling up;
/// presentation uses no floating point, allocation, or unbounded parsing.
pub fn wallpaper(canvas: &mut Canvas<'_>) {
    static WALLPAPER: &[u8; 768000] = include_bytes!("../assets/wallpaper.rgb");
    let step = 640 / canvas.width;
    let remainder = 640 % canvas.width;
    let mut previous = usize::MAX;
    for y in 0..canvas.height {
        let source_y = y * 400 / canvas.height;
        let row = y * canvas.stride;
        if source_y == previous {
            canvas
                .pixels
                .copy_within(row - canvas.stride..row - canvas.stride + canvas.width, row);
        } else {
            let mut source_x = 0;
            let mut error = 0;
            for x in 0..canvas.width {
                let offset = (source_y * 640 + source_x) * 3;
                let color = ((WALLPAPER[offset] as u32) << 16)
                    | ((WALLPAPER[offset + 1] as u32) << 8)
                    | WALLPAPER[offset + 2] as u32;
                canvas.pixels[row + x] = canvas.encode(color);
                source_x += step;
                error += remainder;
                if error >= canvas.width {
                    source_x += 1;
                    error -= canvas.width;
                }
            }
        }
        previous = source_y;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::PixelFormat;
    #[test]
    fn wallpaper_matches_original_sampling_and_preserves_stride_guards() {
        let source = include_bytes!("../assets/wallpaper.rgb");
        for (width, height) in [
            (1, 1),
            (7, 3),
            (639, 399),
            (640, 400),
            (1280, 800),
            (1920, 1080),
            (641, 401),
        ] {
            let stride = width + 3;
            for format in [PixelFormat::Rgb, PixelFormat::Bgr] {
                let mut pixels = std::vec![0xdeadbeef; stride * height + 7];
                wallpaper(&mut Canvas::new(&mut pixels, width, height, stride, format).unwrap());
                for y in 0..height {
                    for x in 0..width {
                        let offset = ((y * 400 / height) * 640 + x * 640 / width) * 3;
                        let rgb = ((source[offset] as u32) << 16)
                            | ((source[offset + 1] as u32) << 8)
                            | source[offset + 2] as u32;
                        let expected = match format {
                            PixelFormat::Bgr => rgb,
                            PixelFormat::Rgb => {
                                rgb.rotate_left(16) & 0xff0000 | rgb & 0xff00 | rgb >> 16
                            }
                        };
                        assert_eq!(
                            pixels[y * stride + x],
                            expected,
                            "{width}x{height} {format:?} ({x},{y})"
                        );
                    }
                    assert!(pixels[y * stride + width..(y + 1) * stride]
                        .iter()
                        .all(|pixel| *pixel == 0xdeadbeef));
                }
                assert!(pixels[stride * height..]
                    .iter()
                    .all(|pixel| *pixel == 0xdeadbeef));
            }
        }
    }
    #[test]
    fn alpha_font_matches_framebuffer_formats_and_keeps_padding() {
        let mut rgb = [0xaabbccdd; 40 * 40];
        let mut bgr = rgb;
        for (pixels, format) in [(&mut rgb, PixelFormat::Rgb), (&mut bgr, PixelFormat::Bgr)] {
            let mut c = Canvas::new(pixels, 32, 32, 40, format).unwrap();
            c.clear(0xf0e0d0);
            text(&mut c, 1, 0, "Aa", 0x193756, Font::Body);
        }
        for y in 0..32 {
            for x in 0..40 {
                let a = rgb[y * 40 + x];
                let b = bgr[y * 40 + x];
                if x < 32 {
                    assert_eq!(a, (b & 0xff00) | ((b & 255) << 16) | ((b >> 16) & 255));
                } else {
                    assert_eq!(a, 0xaabbccdd);
                    assert_eq!(b, 0xaabbccdd);
                }
            }
        }
        assert!(bgr.iter().any(|&p| p != 0xf0e0d0 && p != 0xaabbccdd));
    }
    #[test]
    fn settings_text_scale_changes_glyph_metrics_and_raster_size() {
        use crate::appearance::TextScale;
        let sample = "Settings";
        let regular_width = text_width(sample, Font::Body);
        let large_width = text_width_scaled(sample, Font::Body, TextScale::Percent125);
        assert!(large_width > regular_width);

        let mut regular = [0; 80 * 64];
        let mut large = [0; 80 * 64];
        let mut regular_canvas = Canvas::new(&mut regular, 80, 64, 80, PixelFormat::Bgr).unwrap();
        let mut large_canvas = Canvas::new(&mut large, 80, 64, 80, PixelFormat::Bgr).unwrap();
        regular_canvas.clear(SURFACE);
        large_canvas.clear(SURFACE);
        large_canvas.set_text_scale(TextScale::Percent125);
        text(&mut regular_canvas, 2, 2, sample, TEXT, Font::Body);
        text(&mut large_canvas, 2, 2, sample, TEXT, Font::Body);
        let regular_bounds = raster_bounds(&regular, 80, SURFACE);
        let large_bounds = raster_bounds(&large, 80, SURFACE);
        assert!(large_bounds.2 > regular_bounds.2);
        assert!(large_bounds.3 > regular_bounds.3);
    }

    #[test]
    fn selawik_renders_latin_accents_and_typographic_punctuation() {
        let latin = [
            'à', 'é', 'É', 'ñ', 'Ñ', 'ç', 'ß', 'ø', 'Å', 'æ', 'Æ', 'þ', 'ð', '€', '—', '“', '”',
            '…',
        ];
        for character in latin {
            assert_ne!(
                glyph_index(character),
                glyph_index('?'),
                "U+{:04X}",
                character as u32
            );
        }
        fn same_raster_metrics(left: char, right: char) -> bool {
            let base = Font::Body.index() * GLYPHS_PER_SIZE;
            let left = base + glyph_index(left);
            let right = base + glyph_index(right);
            ADVANCE[left] == ADVANCE[right]
                && X_OFFSET[left] == X_OFFSET[right]
                && ATLAS[left * 768..(left + 1) * 768] == ATLAS[right * 768..(right + 1) * 768]
        }
        assert!(same_raster_metrics('\u{00a0}', ' '));
        assert!(same_raster_metrics('\u{2011}', '-'));
        assert!(same_raster_metrics('\u{201f}', '"'));

        assert!(same_raster_metrics('\u{00aa}', '?'));
        assert!(same_raster_metrics('\u{00ba}', '?'));
        let unsupported = [('字', "字"), ('🙂', "🙂")];
        for (character, sample) in unsupported {
            assert_eq!(glyph_index(character), glyph_index('?'));
            assert_eq!(
                text_width(sample, Font::Body),
                text_width("?", Font::Body),
                "unsupported U+{:04X} keeps a safe fallback advance",
                character as u32
            );
        }
    }

    #[test]
    fn accented_glyph_is_rasterized_and_unsupported_text_uses_visible_question_mark() {
        fn render(sample: &str) -> std::vec::Vec<u32> {
            let mut pixels = std::vec![SURFACE; 64 * 40];
            let mut canvas = Canvas::new(&mut pixels, 64, 40, 64, PixelFormat::Bgr).unwrap();
            text(&mut canvas, 2, 2, sample, TEXT, Font::Body);
            pixels
        }

        let accented = render("é");
        let question = render("?");
        let unsupported = render("字");
        assert_ne!(
            accented, question,
            "accented input gets its own Selawik glyph"
        );
        assert_eq!(
            unsupported, question,
            "unsupported codepoints visibly use '?' raster"
        );
        assert_ne!(
            &ATLAS[(GLYPHS_PER_SIZE + glyph_index('é')) * 768
                ..(GLYPHS_PER_SIZE + glyph_index('é') + 1) * 768],
            &ATLAS[(GLYPHS_PER_SIZE + glyph_index('?')) * 768
                ..(GLYPHS_PER_SIZE + glyph_index('?') + 1) * 768]
        );
    }

    #[test]
    fn generated_unicode_codepoint_table_is_strictly_sorted_and_bounded() {
        let mut previous = 0;
        for index in 0..EXTRA_GLYPH_COUNT {
            let offset = index * 2;
            let codepoint =
                u16::from_le_bytes([EXTRA_CODEPOINTS_LE[offset], EXTRA_CODEPOINTS_LE[offset + 1]]);
            assert!(codepoint >= 0x00a0);
            assert!(codepoint as u32 <= 0x2122);
            if index != 0 {
                assert!(
                    previous < codepoint,
                    "codepoint table must be strictly ordered"
                );
            }
            previous = codepoint;
            assert_eq!(
                glyph_index(char::from_u32(codepoint as u32).unwrap()),
                ASCII_GLYPH_COUNT + index
            );
        }
    }

    fn raster_bounds(
        pixels: &[u32],
        width: usize,
        background: u32,
    ) -> (usize, usize, usize, usize) {
        let mut left = usize::MAX;
        let mut top = usize::MAX;
        let mut right = 0;
        let mut bottom = 0;
        for (index, pixel) in pixels.iter().enumerate() {
            if *pixel == background {
                continue;
            }
            let x = index % width;
            let y = index / width;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
        (left, top, right, bottom)
    }
    #[test]
    fn clipped_surfaces_and_text_preserve_guards_at_extreme_origins() {
        let mut pixels = [0xfeedbeef; 120];
        let mut c = Canvas::new(&mut pixels[8..112], 8, 8, 12, PixelFormat::Bgr).unwrap();
        for origin in [i32::MIN, -20, -2, 7, i32::MAX] {
            rounded_rect(
                &mut c,
                Bounds {
                    x: origin,
                    y: origin,
                    width: 30,
                    height: 30,
                },
                12,
                SURFACE,
            );
            text(&mut c, origin, origin, "Prism", TEXT, Font::Title);
        }
        for y in 0..8 {
            assert!(pixels[8 + y * 12 + 8..8 + y * 12 + 12]
                .iter()
                .all(|&p| p == 0xfeedbeef));
        }
        assert!(pixels[..8]
            .iter()
            .chain(pixels[112..].iter())
            .all(|&p| p == 0xfeedbeef));
    }
}
