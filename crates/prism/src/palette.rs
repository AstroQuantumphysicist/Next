//! Semantic palette translation; raster font coverage and source RGB decode are
//! unchanged. Only canonical UI tokens are translated; source alpha blending
//! must resolve foreground once and decode stored background without resolving.
use crate::appearance::{Accent, Preferences, Theme};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub surface: u32,
    pub raised: u32,
    pub background: u32,
    pub text: u32,
    pub muted: u32,
    pub border: u32,
    pub accent: u32,
    pub accent_soft: u32,
    pub error: u32,
    pub warning: u32,
}
impl Palette {
    pub const DEFAULT: Self = Self {
        surface: 0xf7f9fc,
        raised: 0xffffff,
        background: 0xeaf0f8,
        text: 0x17283f,
        muted: 0x64748b,
        border: 0xcbd5e3,
        accent: 0x2368df,
        accent_soft: 0xe2edff,
        error: 0xb42338,
        warning: 0x965d00,
    };
    pub fn new(preferences: Preferences) -> Self {
        let mut palette = match preferences.theme {
            Theme::Light => Self::DEFAULT,
            Theme::Dark => Self {
                surface: 0x1b1d22,
                raised: 0x24262c,
                background: 0x101114,
                text: 0xf7f8fa,
                muted: 0xb2b7c1,
                border: 0x737985,
                accent: 0x91b5ff,
                accent_soft: 0x263956,
                error: 0xff7a7a,
                warning: 0xffc65b,
            },
            Theme::HighContrast => Self {
                surface: 0x000000,
                raised: 0x000000,
                background: 0x000000,
                text: 0xffffff,
                muted: 0xffffff,
                border: 0xffffff,
                accent: 0xffff00,
                accent_soft: 0x000000,
                error: 0xff8080,
                warning: 0xffff00,
            },
        };
        palette.accent = match (preferences.theme, preferences.accent) {
            (Theme::Light, Accent::Blue) => 0x2368df,
            (Theme::Light, Accent::Violet) => 0x7545c0,
            (Theme::Light, Accent::Teal) => 0x007b78,
            (Theme::Light, Accent::Orange) => 0xa64c00,
            (Theme::Dark, Accent::Blue) => 0x91b5ff,
            (Theme::Dark, Accent::Violet) => 0xc6a4ff,
            (Theme::Dark, Accent::Teal) => 0x65d8cc,
            (Theme::Dark, Accent::Orange) => 0xffb575,
            (Theme::HighContrast, Accent::Blue) => 0xffff00,
            (Theme::HighContrast, Accent::Violet) => 0xffaaff,
            (Theme::HighContrast, Accent::Teal) => 0x00ffff,
            (Theme::HighContrast, Accent::Orange) => 0xffcc88,
        };
        palette
    }
    /// Foreground for a filled accent control; white is suitable only for
    /// the light palette’s dark accent, not pale dark/high-contrast accents.
    pub fn accent_foreground(self) -> u32 {
        if self.surface == Self::DEFAULT.surface {
            0xffffff
        } else {
            0x101114
        }
    }
    /// Translate semantic tokens; legacy bright error red shares the checked
    /// status color. Unrelated literal source pixels remain unchanged.
    pub fn resolve(self, rgb: u32) -> u32 {
        match rgb {
            0xf7f9fc => self.surface,
            0xffffff => self.raised,
            0xeaf0f8 => self.background,
            0x17283f => self.text,
            0x64748b => self.muted,
            0xcbd5e3 => self.border,
            0x2368df => self.accent,
            0xe2edff => self.accent_soft,
            0xb42338 => self.error,
            0x965d00 => self.warning,
            0xdc2626 => self.error,
            _ => rgb,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn luminance(rgb: u32) -> f64 {
        let channel = |shift: u32| {
            let c = ((rgb >> shift) & 255u32) as f64 / 255.;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    fn contrast(a: u32, b: u32) -> f64 {
        let a = luminance(a);
        let b = luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn default_tokens_and_unrelated_rgb_are_retained_while_error_alias_is_checked() {
        let p = Palette::new(Preferences::default());
        assert_eq!(p, Palette::DEFAULT);
        for rgb in [
            0xf7f9fc, 0xffffff, 0xeaf0f8, 0x17283f, 0x64748b, 0xcbd5e3, 0x2368df, 0xe2edff,
            0x123456, 0, 0xb42338, 0x965d00,
        ] {
            assert_eq!(p.resolve(rgb), rgb);
        }
        assert_eq!(p.resolve(0xdc2626), p.error);
        assert!(contrast(p.resolve(0xdc2626), p.background) >= 4.5);
    }
    #[test]
    fn supported_palettes_keep_body_and_accent_text_legible() {
        for theme in [Theme::Light, Theme::Dark, Theme::HighContrast] {
            for accent in [Accent::Blue, Accent::Violet, Accent::Teal, Accent::Orange] {
                let p = Palette::new(Preferences {
                    theme,
                    accent,
                    ..Preferences::default()
                });
                for surface in [p.surface, p.raised, p.background] {
                    assert!(contrast(p.text, surface) >= 7., "body {theme:?}/{accent:?}");
                    for canonical in [0xb42338, 0x965d00, 0xdc2626] {
                        assert!(
                            contrast(p.resolve(canonical), surface) >= 4.5,
                            "status {canonical:x} {theme:?} {}",
                            contrast(p.resolve(canonical), surface)
                        );
                    }
                    assert!(
                        surface == p.background || contrast(p.accent, surface) >= 4.5,
                        "accent {theme:?}/{accent:?} {}",
                        contrast(p.accent, surface)
                    );
                }
                // Current light default muted text is preserved for compatibility;
                // future contrast settings do not claim it meets every AA pairing.
                if theme != Theme::Light {
                    assert!(contrast(p.muted, p.surface) >= 7.);
                }
                assert_eq!(p.resolve(0x2368df), p.accent);
                assert!(contrast(p.accent_foreground(), p.accent) >= 4.5);
            }
        }
    }
}
