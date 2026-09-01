//! Color representation and WCAG 2.x contrast math.
//!
//! Kept free of any Wayland/portal dependency so it's trivially
//! unit-testable and reusable independently of the GUI.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Builds from the (f64, f64, f64) 0.0..=1.0 triple that ashpd's
    /// `Color::pick()` returns.
    pub fn from_unit_floats(r: f64, g: f64, b: f64) -> Self {
        let to_u8 = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::new(to_u8(r), to_u8(g), to_u8(b))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    pub fn from_hex(input: &str) -> anyhow::Result<Self> {
        let s = input.trim().trim_start_matches('#');
        if s.len() != 6 {
            anyhow::bail!("expected a 6-digit hex color like #RRGGBB, got '{input}'");
        }
        let r = u8::from_str_radix(&s[0..2], 16)?;
        let g = u8::from_str_radix(&s[2..4], 16)?;
        let b = u8::from_str_radix(&s[4..6], 16)?;
        Ok(Self::new(r, g, b))
    }

    /// Relative luminance per WCAG 2.x, using sRGB -> linear conversion.
    /// https://www.w3.org/TR/WCAG21/#dfn-relative-luminance
    pub fn relative_luminance(self) -> f64 {
        let lin = |channel: u8| -> f64 {
            let c = channel as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} rgb({}, {}, {})", self.to_hex(), self.r, self.g, self.b)
    }
}

/// WCAG 2.x contrast ratio between two colors, in the range [1.0, 21.0].
/// https://www.w3.org/TR/WCAG21/#dfn-contrast-ratio
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let la = a.relative_luminance();
    let lb = b.relative_luminance();
    let (lighter, darker) = if la >= lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WcagLevel {
    Fail,
    AaLargeOnly,
    Aa,
    Aaa,
}

impl fmt::Display for WcagLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            WcagLevel::Fail => "FAIL",
            WcagLevel::AaLargeOnly => "AA (large text only)",
            WcagLevel::Aa => "AA",
            WcagLevel::Aaa => "AAA",
        };
        write!(f, "{s}")
    }
}

/// Classifies a contrast ratio for normal-size text against the WCAG 2.x
/// thresholds (4.5:1 for AA, 7:1 for AAA; large text is 3:1 / 4.5:1).
pub fn wcag_level(ratio: f64, large_text: bool) -> WcagLevel {
    if large_text {
        if ratio >= 4.5 {
            WcagLevel::Aaa
        } else if ratio >= 3.0 {
            WcagLevel::Aa
        } else {
            WcagLevel::Fail
        }
    } else if ratio >= 7.0 {
        WcagLevel::Aaa
    } else if ratio >= 4.5 {
        WcagLevel::Aa
    } else if ratio >= 3.0 {
        WcagLevel::AaLargeOnly
    } else {
        WcagLevel::Fail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = Rgb::new(0x1A, 0x2B, 0x3C);
        assert_eq!(c.to_hex(), "#1A2B3C");
        assert_eq!(Rgb::from_hex("#1A2B3C").unwrap(), c);
        assert_eq!(Rgb::from_hex("1a2b3c").unwrap(), c);
    }

    #[test]
    fn from_hex_rejects_bad_input() {
        assert!(Rgb::from_hex("#12345").is_err());
        assert!(Rgb::from_hex("#zzzzzz").is_err());
    }

    #[test]
    fn black_and_white_have_max_contrast() {
        let ratio = contrast_ratio(Rgb::new(0, 0, 0), Rgb::new(255, 255, 255));
        assert!((ratio - 21.0).abs() < 0.01);
    }

    #[test]
    fn identical_colors_have_contrast_one() {
        let c = Rgb::new(100, 150, 200);
        let ratio = contrast_ratio(c, c);
        assert!((ratio - 1.0).abs() < 0.0001);
    }

    #[test]
    fn contrast_ratio_is_order_independent() {
        let a = Rgb::new(20, 30, 40);
        let b = Rgb::new(220, 210, 200);
        assert!((contrast_ratio(a, b) - contrast_ratio(b, a)).abs() < 1e-9);
    }

    #[test]
    fn wcag_level_thresholds_normal_text() {
        assert_eq!(wcag_level(2.0, false), WcagLevel::Fail);
        assert_eq!(wcag_level(3.5, false), WcagLevel::AaLargeOnly);
        assert_eq!(wcag_level(4.5, false), WcagLevel::Aa);
        assert_eq!(wcag_level(7.0, false), WcagLevel::Aaa);
    }

    #[test]
    fn wcag_level_thresholds_large_text() {
        assert_eq!(wcag_level(2.9, true), WcagLevel::Fail);
        assert_eq!(wcag_level(3.0, true), WcagLevel::Aa);
        assert_eq!(wcag_level(4.5, true), WcagLevel::Aaa);
    }

    #[test]
    fn from_unit_floats_matches_portal_range() {
        let c = Rgb::from_unit_floats(1.0, 0.0, 0.5019607843137255);
        assert_eq!(c, Rgb::new(255, 0, 128));
    }
}