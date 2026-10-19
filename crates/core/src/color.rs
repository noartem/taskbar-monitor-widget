//! Colors and the palette derived from theme settings.

use crate::config::{Appearance, ThemeMode};
use crate::metrics::MetricCategory;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.0);
    pub const WHITE: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }

    /// `#RGB`, `#RRGGBB` or `#RRGGBBAA` (the `#` is optional).
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(h.get(i..i + 1)?, 16).ok().map(|v| v * 17);
                Some(Self::from_u8(n(0)?, n(1)?, n(2)?))
            }
            6 => Some(Self::from_u8(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Self::from_u8(byte(0)?, byte(2)?, byte(4)?).with_alpha(byte(6)? as f32 / 255.0)),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if self.a >= 0.999 {
            format!("#{:02X}{:02X}{:02X}", c(self.r), c(self.g), c(self.b))
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", c(self.r), c(self.g), c(self.b), c(self.a))
        }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a: a.clamp(0.0, 1.0), ..self }
    }

    pub fn mul_alpha(self, k: f32) -> Self {
        self.with_alpha(self.a * k)
    }

    /// Linear interpolation towards `other`.
    pub fn mix(self, other: Rgba, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self::new(l(self.r, other.r), l(self.g, other.g), l(self.b, other.b), l(self.a, other.a))
    }

    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }
}

/// Resolved colors for drawing.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub foreground: Rgba,
    pub label: Rgba,
    pub background: Rgba,
    /// Background of bars, rings and graph areas.
    pub track: Rgba,
    pub accent: Rgba,
    pub warn: Rgba,
    pub crit: Rgba,
    /// Flyout card background and border.
    pub surface: Rgba,
    pub surface_border: Rgba,
    pub color_by_category: bool,
}

impl Palette {
    /// `system_dark` is the taskbar theme; `system_accent` the Windows accent color.
    pub fn build(a: &Appearance, system_dark: bool, system_accent: Option<Rgba>) -> Self {
        let dark = match a.theme {
            ThemeMode::System => system_dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::Custom => Rgba::parse(&a.foreground_color).map(|c| c.luminance() > 0.5).unwrap_or(system_dark),
        };
        let foreground = match a.theme {
            ThemeMode::Custom => Rgba::parse(&a.foreground_color).unwrap_or(Rgba::WHITE),
            _ if dark => Rgba::WHITE,
            _ => Rgba::from_u8(0x1A, 0x1A, 0x1A),
        };
        let bg_base = match a.theme {
            ThemeMode::Custom => Rgba::parse(&a.background_color),
            _ if dark => Some(Rgba::from_u8(0x20, 0x20, 0x20)),
            _ => Some(Rgba::from_u8(0xF3, 0xF3, 0xF3)),
        }
        .unwrap_or(Rgba::BLACK);

        let custom_accent = Rgba::parse(&a.accent_color);
        let accent = if a.use_system_accent { system_accent.or(custom_accent) } else { custom_accent.or(system_accent) }
            .unwrap_or(Rgba::from_u8(0x4C, 0xC2, 0xFF));
        // Accent colors from Windows are often too dark on a dark taskbar; lift them for contrast.
        let accent = match (dark, accent.luminance()) {
            (true, l) if l < 0.35 => accent.mix(Rgba::WHITE, 0.35),
            (false, l) if l > 0.75 => accent.mix(Rgba::BLACK, 0.3),
            _ => accent,
        };

        Self {
            dark,
            foreground,
            label: foreground.mul_alpha(a.label_opacity),
            background: bg_base.with_alpha(a.background_opacity),
            track: foreground.with_alpha(if dark { 0.16 } else { 0.14 }),
            accent,
            warn: Rgba::parse(&a.warn_color).unwrap_or(Rgba::from_u8(0xFF, 0xB9, 0x00)),
            crit: Rgba::parse(&a.crit_color).unwrap_or(Rgba::from_u8(0xF1, 0x70, 0x7A)),
            surface: if dark { Rgba::from_u8(0x2B, 0x2B, 0x2B) } else { Rgba::from_u8(0xFB, 0xFB, 0xFB) },
            surface_border: foreground.with_alpha(0.08),
            color_by_category: a.color_by_category,
        }
    }

    pub fn category(&self, category: MetricCategory) -> Rgba {
        if !self.color_by_category {
            return self.accent;
        }
        let (dark, light) = match category {
            MetricCategory::Cpu => ((0x4C, 0xC2, 0xFF), (0x00, 0x67, 0xC0)),
            MetricCategory::Memory => ((0xB4, 0x8E, 0xF5), (0x7A, 0x3E, 0xC8)),
            MetricCategory::Gpu => ((0x6C, 0xCB, 0x5F), (0x10, 0x7C, 0x10)),
            MetricCategory::Network => ((0xF7, 0xA4, 0x5C), (0xC2, 0x5A, 0x00)),
            MetricCategory::Disk => ((0x5E, 0xD5, 0xC8), (0x00, 0x7F, 0x73)),
            MetricCategory::Claude => ((0xE8, 0x8B, 0x6A), (0xC1, 0x5F, 0x3C)),
            MetricCategory::Sensors => ((0xF0, 0xC0, 0x40), (0x9D, 0x5D, 0x00)),
        };
        let (r, g, b) = if self.dark { dark } else { light };
        Rgba::from_u8(r, g, b)
    }

    /// Value color after applying warning/critical thresholds.
    pub fn threshold(&self, base: Rgba, value: f64, a: &Appearance) -> Rgba {
        if !a.use_threshold_colors || !value.is_finite() {
            base
        } else if value >= a.crit_threshold as f64 {
            self.crit
        } else if value >= a.warn_threshold as f64 {
            self.warn
        } else {
            base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex() {
        assert_eq!(Rgba::parse("#fff"), Some(Rgba::WHITE));
        assert_eq!(Rgba::parse("000000"), Some(Rgba::BLACK));
        assert_eq!(Rgba::parse("#FF000080").unwrap().to_hex(), "#FF000080");
        assert!(Rgba::parse("#12345").is_none());
    }

    #[test]
    fn palette_follows_system_theme() {
        let a = Appearance::default();
        assert!(Palette::build(&a, true, None).dark);
        assert!(!Palette::build(&a, false, None).dark);
        let light = Appearance { theme: ThemeMode::Light, ..Appearance::default() };
        assert!(!Palette::build(&light, true, None).dark);
    }
}
