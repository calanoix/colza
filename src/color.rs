//! Color representation, conversions (HSL, OkLCh) and contrast math
//! (WCAG 2.x ratio, APCA Lc).
//!
//! Kept free of any Wayland/portal dependency so it's trivially
//! unit-testable and reusable independently of the GUI.
//!
//! `Rgb` (8-bit sRGB) is the one color that is ever *stored*: it's what
//! the screen gives us and what WCAG's math is defined on. `Hsl` and
//! `Oklch` are views onto it, converted on demand, and they use `f64`
//! throughout so a round trip through them doesn't lose anything before
//! the final rounding back to 8 bits.

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

    /// Builds a color from channels in `[0, 1]` (rounded, clamped).
    pub fn from_unit_floats(r: f64, g: f64, b: f64) -> Self {
        let q = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::new(q(r), q(g), q(b))
    }

    /// Channels in `[0, 1]`, still sRGB-encoded (gamma), not linear.
    fn to_unit_floats(self) -> [f64; 3] {
        [self.r, self.g, self.b].map(|c| c as f64 / 255.0)
    }

    /// Relative luminance per WCAG 2.x, using sRGB -> linear conversion.
    /// https://www.w3.org/TR/WCAG21/#dfn-relative-luminance
    pub fn relative_luminance(self) -> f64 {
        let [r, g, b] = self.to_unit_floats().map(srgb_to_linear);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    pub fn to_hsl(self) -> Hsl {
        let [r, g, b] = self.to_unit_floats();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;

        if d == 0.0 {
            // Gray: hue is undefined, and 0 is the conventional stand-in
            // (CSS serializes it the same way).
            return Hsl { h: 0.0, s: 0.0, l };
        }

        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } * 60.0;

        Hsl { h, s, l }
    }

    /// HSL covers exactly the sRGB cube, so unlike `from_oklch` there is
    /// no gamut to map into — only rounding to 8 bits.
    pub fn from_hsl(hsl: Hsl) -> Self {
        let Hsl { h, s, l } = hsl;
        let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
        // CSS Color 4's `hslToRgb`: one branch-free formula per channel.
        let f = |n: f64| {
            let k = (n + h.rem_euclid(360.0) / 30.0) % 12.0;
            let a = s * l.min(1.0 - l);
            l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
        };
        Self::from_unit_floats(f(0.0), f(8.0), f(4.0))
    }

    pub fn to_oklch(self) -> Oklch {
        let [r, g, b] = self.to_unit_floats().map(srgb_to_linear);
        let [l, a, b] = linear_srgb_to_oklab([r, g, b]);
        let c = (a * a + b * b).sqrt();
        // Grays come out of the matrices with a tiny nonzero chroma
        // (~1e-8) whose angle is pure rounding noise. Reporting it would
        // make the hue slider jump around for neutral colors, so treat
        // anything this small as achromatic, as CSS does with `none`.
        if c < ACHROMATIC_CHROMA {
            return Oklch { l, c: 0.0, h: 0.0 };
        }
        Oklch { l, c, h: b.atan2(a).to_degrees().rem_euclid(360.0) }
    }

    /// Converts to sRGB, mapping out-of-gamut colors into it, and says
    /// whether the color was really outside sRGB, so the UI can say so
    /// instead of silently showing a different color than requested.
    ///
    /// Most of the OkLCh space lies outside sRGB (high chroma at most
    /// hues, and anything bright *and* saturated). Clamping each channel
    /// independently would be the obvious fix, but it visibly shifts hue
    /// and lightness — a saturated blue slides toward purple. Instead
    /// this follows CSS Color 4's gamut-mapping algorithm, i.e. what a
    /// browser does with the same `oklch()`: keep L and h, lower chroma
    /// until the color fits.
    /// https://www.w3.org/TR/css-color-4/#binsearch
    ///
    /// "Really" outside because colors on the gamut's surface routinely
    /// come back a hair outside it — a channel at -0.0004 — just from
    /// rounding (the decimals of the text field, or float noise). Those
    /// count as in gamut: off by less than half an 8-bit step, they
    /// quantize to the same color as their clipped version, so mapping
    /// them would only invent a difference.
    pub fn from_oklch_mapped(lch: Oklch) -> (Self, bool) {
        let l = lch.l;

        // Outside (0, 1) lightness there is nothing left to desaturate
        // toward; CSS maps these to white/black outright.
        if l >= 1.0 {
            return (Self::new(255, 255, 255), lch.c > ACHROMATIC_CHROMA || l > 1.0);
        }
        if l <= 0.0 {
            return (Self::new(0, 0, 0), lch.c > ACHROMATIC_CHROMA || l < 0.0);
        }

        let at_chroma = |c: f64| Oklch { l, c, h: lch.h };
        // Sign-preserving transfer function (as CSS Color 4 defines it),
        // so a channel that falls below 0 stays visibly negative and the
        // gamut checks below can see it.
        let encoded = |lch: Oklch| {
            oklab_to_linear_srgb(oklch_to_oklab(lch)).map(|c| c.signum() * linear_to_srgb(c.abs()))
        };
        let quantize = |[r, g, b]: [f64; 3]| Self::from_unit_floats(r, g, b);

        let origin = at_chroma(lch.c.max(0.0));
        let rgb = encoded(origin);
        const HALF_STEP: f64 = 0.5 / 255.0;
        if rgb.iter().all(|&c| (-HALF_STEP..=1.0 + HALF_STEP).contains(&c)) {
            return (quantize(rgb), false);
        }

        // The spec's binary search on chroma. The twist over a plain
        // bisection: at each step, if simply clipping the candidate lands
        // within a just-noticeable difference of it, the clipped color is
        // accepted. That stops the search from desaturating further than
        // the eye can tell apart, which plain bisection to the exact
        // boundary would do.
        const JND: f64 = 0.02;
        const EPSILON: f64 = 0.0001;
        let clip = |rgb: [f64; 3]| rgb.map(|c| c.clamp(0.0, 1.0));
        let delta_e = |a: Oklch, clipped: [f64; 3]| {
            let [l1, a1, b1] = oklch_to_oklab(a);
            let [l2, a2, b2] = linear_srgb_to_oklab(clipped.map(srgb_to_linear));
            ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
        };

        let mut clipped = clip(rgb);
        if delta_e(origin, clipped) < JND {
            return (quantize(clipped), true);
        }
        let (mut min, mut max) = (0.0, origin.c);
        let mut min_in_gamut = true;
        while max - min > EPSILON {
            let chroma = (min + max) / 2.0;
            let current = at_chroma(chroma);
            let rgb = encoded(current);
            if min_in_gamut && rgb.iter().all(|&c| (0.0..=1.0).contains(&c)) {
                min = chroma;
                continue;
            }
            clipped = clip(rgb);
            let e = delta_e(current, clipped);
            if e < JND {
                if JND - e < EPSILON {
                    break;
                }
                min_in_gamut = false;
                min = chroma;
            } else {
                max = chroma;
            }
        }
        (quantize(clipped), true)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} rgb({}, {}, {})", self.to_hex(), self.r, self.g, self.b)
    }
}

/// HSL as in CSS: `h` in degrees `[0, 360)`, `s` and `l` in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsl {
    pub h: f64,
    pub s: f64,
    pub l: f64,
}

impl Hsl {
    /// `hsl(210, 50%, 40.5%)`: CSS's comma syntax, matching the `rgb(...)`
    /// format next to it in the UI.
    ///
    /// One decimal everywhere is the least that survives a round trip
    /// (measured over all 16.7 M colors; see the exhaustive test below).
    /// With whole numbers, ~3.6 M HSL triples can't name 16.7 M sRGB
    /// colors, so copying the text and pasting it back — here or in a
    /// browser — would land on a neighbor, and near a threshold that can
    /// flip a pass into a fail. Trailing zeros are dropped, so typical
    /// values still read `hsl(210, 50%, 40%)`.
    pub fn to_css(self) -> String {
        let h = normalize_hue(self.h, 1);
        format!(
            "hsl({}, {}%, {}%)",
            fmt_num(h, 1),
            fmt_num(self.s * 100.0, 1),
            fmt_num(self.l * 100.0, 1)
        )
    }

    /// Accepts the comma syntax `hsl(210, 50%, 40%)`, the space syntax
    /// `hsl(210deg 50% 40%)`, and `hsla(...)`, which CSS treats as an
    /// alias. Percent signs on S/L are optional, as in CSS Color 4.
    /// An alpha component (`/ 0.5` or a 4th value) is rejected rather than
    /// dropped: the alpha slider is the only alpha this app has, and
    /// silently ignoring a typed one would be surprising.
    pub fn parse_css(text: &str) -> Option<Self> {
        let [h, s, l] = css_args(text, &["hsl", "hsla"])?;
        let h = parse_hue(h)?;
        let s = parse_percent(s)?;
        let l = parse_percent(l)?;
        if !(0.0..=100.0).contains(&s) || !(0.0..=100.0).contains(&l) {
            return None;
        }
        Some(Self { h, s: s / 100.0, l: l / 100.0 })
    }
}

/// OkLCh (Björn Ottosson's OkLab in polar form), as in CSS: `l` in
/// `[0, 1]`, `c` from 0 up (sRGB tops out around 0.32, and CSS maps
/// `100%` to 0.4), `h` in degrees `[0, 360)`.
///
/// Its point here, over HSL, is that `l` tracks perceived lightness:
/// two colors with the same OkLCh `l` look about equally light, which is
/// a much better guide when hunting for a passing contrast than HSL's
/// `l`, where yellow and blue at "50%" are worlds apart. (It is still not
/// WCAG's luminance — the ratio shown is the only authority.)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

impl Oklch {
    /// `oklch(62.8% 0.25768 29.23)`, CSS's only syntax for this function.
    ///
    /// The precision (L to 0.01%, C to 5 decimals, h to 0.01°) is the
    /// least that round-trips every 8-bit sRGB color, measured
    /// exhaustively: the more usual `62.8% 0.258 29.2` misses ~3.7 M of
    /// them (mostly saturated darks, where C and h move fast per 8-bit
    /// step). Exact matters here for the same reason as in
    /// [`Hsl::to_css`]: the text is something users copy into CSS and
    /// paste back, and the ratio must not move when they do.
    pub fn to_css(self) -> String {
        let c = round_to(self.c, 5);
        // A chroma that prints as 0 has no meaningful hue; print 0 rather
        // than a leftover angle that would read as information.
        let h = if c == 0.0 { 0.0 } else { normalize_hue(self.h, 2) };
        format!(
            "oklch({}% {} {})",
            fmt_num(self.l * 100.0, 2),
            fmt_num(c, 5),
            fmt_num(h, 2)
        )
    }

    /// Accepts CSS's forms: lightness as `62.8%` or `0.628`, chroma as a
    /// number or a percentage (`100%` = 0.4), hue as a number, `deg`, or
    /// `none`. Commas are tolerated (CSS doesn't allow them here, but
    /// pasting from a comma-separated list shouldn't fail for that
    /// alone). Alpha is rejected, as for [`Hsl::parse_css`].
    pub fn parse_css(text: &str) -> Option<Self> {
        let [l, c, h] = css_args(text, &["oklch"])?;
        let l = match l.strip_suffix('%') {
            Some(pct) => pct.trim().parse::<f64>().ok()? / 100.0,
            None => l.parse::<f64>().ok()?,
        };
        let c = match c.strip_suffix('%') {
            Some(pct) => pct.trim().parse::<f64>().ok()? / 100.0 * 0.4,
            None => c.parse::<f64>().ok()?,
        };
        let h = if h.eq_ignore_ascii_case("none") { 0.0 } else { parse_hue(h)? };
        if !l.is_finite() || !c.is_finite() || c < 0.0 {
            return None;
        }
        // CSS clamps L to [0, 1] at parse time rather than rejecting it.
        Some(Self { l: l.clamp(0.0, 1.0), c, h })
    }
}

/// Chroma under which an OkLCh color counts as a gray.
///
/// Comfortably above the matrix round-off for 8-bit grays (~1e-7) and far
/// below the smallest chroma a non-gray 8-bit color has (~2e-3).
const ACHROMATIC_CHROMA: f64 = 1e-5;

/// WCAG 2.x contrast ratio between two colors, in the range [1.0, 21.0].
/// https://www.w3.org/TR/WCAG21/#dfn-contrast-ratio
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let la = a.relative_luminance();
    let lb = b.relative_luminance();
    let (lighter, darker) = if la >= lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

/// APCA lightness contrast (Lc) of `text` on `bg`, roughly -108 to 106.
///
/// Port of `APCAcontrast(sRGBtoY(text), sRGBtoY(bg))` from the reference
/// implementation, `apca-w3` 0.1.9 (APCA 0.0.98G-4g constants),
/// checked against its own test values below.
///
/// Unlike the WCAG ratio, the order matters: APCA models text and its
/// background differently, and the sign gives the polarity — positive
/// for dark text on a light background, negative for light on dark. The
/// level thresholds apply to the magnitude.
pub fn apca_contrast(text: Rgb, bg: Rgb) -> f64 {
    const BLK_THRS: f64 = 0.022;
    const BLK_CLMP: f64 = 1.414;
    const DELTA_Y_MIN: f64 = 0.0005;
    const LO_CLIP: f64 = 0.1;
    const SCALE: f64 = 1.14;
    const LO_OFFSET: f64 = 0.027;

    // Soft clamp toward black: APCA's model of flare and how little the
    // eye distinguishes among near-blacks.
    let soft_clamp = |y: f64| {
        if y > BLK_THRS { y } else { y + (BLK_THRS - y).powf(BLK_CLMP) }
    };
    let txt = soft_clamp(text.apca_y());
    let bg = soft_clamp(bg.apca_y());

    if (bg - txt).abs() < DELTA_Y_MIN {
        return 0.0;
    }
    let lc = if bg > txt {
        // Normal polarity: dark text on light.
        let sapc = (bg.powf(0.56) - txt.powf(0.57)) * SCALE;
        if sapc < LO_CLIP { 0.0 } else { sapc - LO_OFFSET }
    } else {
        // Reverse polarity: light text on dark.
        let sapc = (bg.powf(0.65) - txt.powf(0.62)) * SCALE;
        if sapc > -LO_CLIP { 0.0 } else { sapc + LO_OFFSET }
    };
    lc * 100.0
}

impl Rgb {
    /// APCA's screen luminance estimate. Deliberately *not*
    /// [`Self::relative_luminance`]: APCA uses a plain 2.4 power curve
    /// (no linear toe) and slightly different coefficients, to model a
    /// real monitor rather than the sRGB spec. Mixing the two up shifts
    /// Lc by several points on dark colors.
    fn apca_y(self) -> f64 {
        let ch = |c: u8| (c as f64 / 255.0).powf(2.4);
        0.2126729 * ch(self.r) + 0.7151522 * ch(self.g) + 0.0721750 * ch(self.b)
    }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// Matrices from Björn Ottosson, "A perceptual color space for image
// processing" (https://bottosson.github.io/posts/oklab/) — the same
// space CSS's `oklch()` is defined on.

fn linear_srgb_to_oklab([r, g, b]: [f64; 3]) -> [f64; 3] {
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn oklab_to_linear_srgb([l, a, b]: [f64; 3]) -> [f64; 3] {
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;
    let (l, m, s) = (l_.powi(3), m_.powi(3), s_.powi(3));
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

fn oklch_to_oklab(lch: Oklch) -> [f64; 3] {
    let h = lch.h.to_radians();
    [lch.l, lch.c * h.cos(), lch.c * h.sin()]
}

/// The three arguments of a CSS color function named one of `names`,
/// split on commas and/or whitespace. `None` for a different function,
/// a missing paren, an alpha part, or a count other than three.
fn css_args<'a>(text: &'a str, names: &[&str]) -> Option<[&'a str; 3]> {
    let t = text.trim();
    let (name, rest) = t.split_once('(')?;
    if !names.iter().any(|n| name.trim().eq_ignore_ascii_case(n)) {
        return None;
    }
    let inner = rest.trim_end().strip_suffix(')')?;
    if inner.contains('/') {
        return None;
    }
    let mut parts = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty());
    let args = [parts.next()?, parts.next()?, parts.next()?];
    parts.next().is_none().then_some(args)
}

/// A hue as a plain number or with `deg`, normalized to `[0, 360)`.
/// Other angle units (`turn`, `rad`) are rare enough in hand-typed colors
/// that they're simply not accepted.
fn parse_hue(s: &str) -> Option<f64> {
    let n = s
        .strip_suffix("deg")
        .unwrap_or(s)
        .trim()
        .parse::<f64>()
        .ok()?;
    n.is_finite().then(|| n.rem_euclid(360.0))
}

fn parse_percent(s: &str) -> Option<f64> {
    let n = s.strip_suffix('%').unwrap_or(s).trim().parse::<f64>().ok()?;
    n.is_finite().then_some(n)
}

fn round_to(v: f64, decimals: i32) -> f64 {
    let k = 10f64.powi(decimals);
    (v * k).round() / k
}

/// Hue rounded for display, with a value that rounds up to 360 shown as 0
/// (359.97 -> "0", not "360").
fn normalize_hue(h: f64, decimals: i32) -> f64 {
    let h = round_to(h.rem_euclid(360.0), decimals);
    if h >= 360.0 { 0.0 } else { h }
}

/// `v` with at most `decimals` decimals and no trailing zeros, so the
/// field reads `50%` rather than `50.0%`. Never prints `-0`.
pub fn fmt_num(v: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v);
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
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
    fn from_unit_floats_matches_portal_range() {
        let c = Rgb::from_unit_floats(1.0, 0.0, 0.5019607843137255);
        assert_eq!(c, Rgb::new(255, 0, 128));
    }

    /// The reference implementation's own test pairs (apca-w3 0.1.9,
    /// `test/index.js`), text first.
    #[test]
    fn apca_matches_reference() {
        let hex = |s: &str| {
            let s = s.trim_start_matches('#');
            let d: Vec<u8> = s.bytes().map(|b| (b as char).to_digit(16).unwrap() as u8 * 17).collect();
            Rgb::new(d[0], d[1], d[2])
        };
        for (txt, bg, want) in [
            ("#888", "#fff", 63.056469930209424),
            ("#fff", "#888", -68.54146436644962),
            ("#000", "#aaa", 58.146262578561334),
            ("#aaa", "#000", -56.24113336839742),
            ("#123", "#def", 91.66830811481631),
            ("#def", "#123", -93.06770049484275),
            ("#123", "#444", 8.32326136957393),
            ("#444", "#123", -7.526878460278154),
        ] {
            let got = apca_contrast(hex(txt), hex(bg));
            assert!((got - want).abs() < 1e-9, "{txt} on {bg}: {got} != {want}");
        }
        assert_eq!(apca_contrast(Rgb::new(9, 9, 9), Rgb::new(9, 9, 9)), 0.0);
    }

    // --- HSL ---

    #[test]
    fn hsl_known_values() {
        let h = Rgb::new(255, 0, 0).to_hsl();
        assert_eq!((h.h, h.s, h.l), (0.0, 1.0, 0.5));
        assert_eq!(Rgb::new(0x33, 0x66, 0x99).to_hsl().to_css(), "hsl(210, 50%, 40%)");
        assert_eq!(Rgb::new(128, 128, 128).to_hsl().to_css(), "hsl(0, 0%, 50.2%)");
        assert_eq!(Rgb::from_hsl(Hsl { h: 120.0, s: 1.0, l: 0.25 }), Rgb::new(0, 128, 0));
    }

    #[test]
    fn hsl_parse_variants() {
        let want = Some(Hsl { h: 210.0, s: 0.5, l: 0.4 });
        assert_eq!(Hsl::parse_css("hsl(210, 50%, 40%)"), want);
        assert_eq!(Hsl::parse_css("  HSL(210deg 50% 40%) "), want);
        assert_eq!(Hsl::parse_css("hsla(210, 50, 40)"), want);
        assert_eq!(Hsl::parse_css("hsl(570, 50%, 40%)"), want); // wraps
        assert_eq!(Hsl::parse_css("hsl(210 50% 40% / 0.5)"), None);
        assert_eq!(Hsl::parse_css("hsl(210, 50%)"), None);
        assert_eq!(Hsl::parse_css("hsl(210, 150%, 40%)"), None);
        assert_eq!(Hsl::parse_css("rgb(1, 2, 3)"), None);
    }

    /// The text written for an 8-bit color parses back to that exact
    /// color: what the user copies out of the field is what they picked.
    #[test]
    fn hsl_css_roundtrips() {
        hsl_roundtrip(5);
    }

    #[test]
    #[ignore = "exhaustive (16.7 M colors): cargo test --release -- --ignored"]
    fn hsl_css_roundtrips_every_color() {
        hsl_roundtrip(1);
    }

    fn hsl_roundtrip(stride: usize) {
        for_colors(stride, |c| {
            let back = Rgb::from_hsl(Hsl::parse_css(&c.to_hsl().to_css()).unwrap());
            assert_eq!(back, c, "{}", c.to_hsl().to_css());
        });
    }

    // --- OkLCh ---

    #[test]
    fn oklch_known_values() {
        // Reference values from CSS Color 4 / oklch.com.
        assert_eq!(Rgb::new(255, 0, 0).to_oklch().to_css(), "oklch(62.8% 0.25768 29.23)");
        assert_eq!(Rgb::new(255, 255, 255).to_oklch().to_css(), "oklch(100% 0 0)");
        assert_eq!(Rgb::new(0, 0, 0).to_oklch().to_css(), "oklch(0% 0 0)");
        assert_eq!(Rgb::new(0, 0, 255).to_oklch().to_css(), "oklch(45.2% 0.31321 264.05)");
    }

    #[test]
    fn grays_have_no_chroma_or_hue() {
        for v in 0..=255 {
            let lch = Rgb::new(v, v, v).to_oklch();
            assert_eq!((lch.c, lch.h), (0.0, 0.0), "gray {v}");
        }
    }

    #[test]
    fn oklch_parse_variants() {
        let p = |s| Oklch::parse_css(s);
        assert_eq!(p("oklch(62.8% 0.258 29.2)"), Some(Oklch { l: 0.628, c: 0.258, h: 29.2 }));
        assert_eq!(p("oklch(0.5 0.1 120deg)"), Some(Oklch { l: 0.5, c: 0.1, h: 120.0 }));
        assert_eq!(p("oklch(50% 50% none)"), Some(Oklch { l: 0.5, c: 0.2, h: 0.0 }));
        assert_eq!(p("oklch(50% 0.1 120 / 50%)"), None);
        assert_eq!(p("oklch(50% -0.1 120)"), None);
        assert_eq!(p("lch(50% 30 120)"), None);
    }

    #[test]
    fn oklch_css_roundtrips() {
        oklch_roundtrip(5);
    }

    /// This is the test that fixed `Oklch::to_css`'s precision: it fails
    /// on hundreds of thousands of colors with one decimal less anywhere.
    #[test]
    #[ignore = "exhaustive (16.7 M colors): cargo test --release -- --ignored"]
    fn oklch_css_roundtrips_every_color() {
        oklch_roundtrip(1);
    }

    fn oklch_roundtrip(stride: usize) {
        for_colors(stride, |c| {
            let css = c.to_oklch().to_css();
            let (back, mapped) = Rgb::from_oklch_mapped(Oklch::parse_css(&css).unwrap());
            assert_eq!(back, c, "{css}");
            // An sRGB color must never be reported as out of sRGB, even
            // though rounding the text nudges edge colors a hair outside.
            assert!(!mapped, "{css} flagged out of gamut");
        });
    }

    #[test]
    fn out_of_gamut_keeps_hue_and_lightness() {
        // Far more chroma than sRGB has at this hue.
        let want = Oklch { l: 0.6, c: 0.4, h: 145.0 };
        let (rgb, mapped) = Rgb::from_oklch_mapped(want);
        assert!(mapped);
        // Not exact: the spec's algorithm accepts a clipped color within a
        // just-noticeable difference (ΔE_OK 0.02) of the in-gamut target,
        // so L and h may drift by about that much — as in a browser.
        let got = rgb.to_oklch();
        assert!((got.l - want.l).abs() < 0.02, "{got:?}");
        assert!((got.h - want.h).abs() < 3.0, "{got:?}");
        assert!(got.c < want.c);

        // In-gamut colors are left alone.
        let (_, mapped) = Rgb::from_oklch_mapped(Oklch { l: 0.6, c: 0.05, h: 145.0 });
        assert!(!mapped);
    }

    #[test]
    fn fmt_num_trims() {
        assert_eq!(fmt_num(50.0, 1), "50");
        assert_eq!(fmt_num(40.54, 1), "40.5");
        assert_eq!(fmt_num(-0.01, 1), "0");
        assert_eq!(fmt_num(0.2577, 3), "0.258");
        assert_eq!(fmt_num(0.1, 5), "0.1");
    }

    /// Every `stride`-th value per channel. 255 is divisible by 1, 3, 5,
    /// 15…, so those strides always include both ends of each channel —
    /// where the gamut edges, and most rounding trouble, are.
    fn for_colors(stride: usize, mut f: impl FnMut(Rgb)) {
        assert_eq!(255 % stride, 0, "stride must divide 255");
        for r in (0..=255).step_by(stride) {
            for g in (0..=255).step_by(stride) {
                for b in (0..=255).step_by(stride) {
                    f(Rgb::new(r, g, b));
                }
            }
        }
    }
}