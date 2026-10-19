//! Pure sRGB colour math for deriving readable GUI roles from TUI palettes.
//!
//! Channels are kept as `f64` in `0.0..=255.0` so alpha compositing matches
//! the GUI contrast tests (`gui/tests/contrast.ts`), which never round an
//! intermediate composite. Only colours that are emitted as tokens are
//! rounded, and every floor is checked on the rounded value, so a token that
//! passes here passes after it is written out as `#rrggbb`.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub f64, pub f64, pub f64);

pub const BLACK: Rgb = Rgb(0.0, 0.0, 0.0);
pub const WHITE: Rgb = Rgb(255.0, 255.0, 255.0);

impl Rgb {
    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self(f64::from(r), f64::from(g), f64::from(b))
    }

    /// `#rrggbb` (alpha 1) or `#rrggbbaa`.
    pub fn parse(value: &str) -> Option<(Self, f64)> {
        let digits = value.strip_prefix('#')?;
        if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
        let rgb = Self::from_u8(byte(0)?, byte(2)?, byte(4)?);
        let alpha = if digits.len() == 8 {
            f64::from(byte(6)?) / 255.0
        } else {
            1.0
        };
        Some((rgb, alpha))
    }

    pub fn rounded(self) -> Self {
        let c = |v: f64| v.round().clamp(0.0, 255.0);
        Self(c(self.0), c(self.1), c(self.2))
    }

    pub fn hex(self) -> String {
        let Self(r, g, b) = self.rounded();
        format!("#{:02x}{:02x}{:02x}", r as u8, g as u8, b as u8)
    }
}

/// `fg` at `alpha` composited over an opaque `bg`.
pub fn over(fg: Rgb, alpha: f64, bg: Rgb) -> Rgb {
    mix(bg, fg, alpha)
}

/// Linear blend in sRGB: `t = 0` is `a`, `t = 1` is `b`.
pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let l = |x: f64, y: f64| x + (y - x) * t;
    Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// WCAG 2 relative luminance.
pub fn relative_luminance(c: Rgb) -> f64 {
    let lin = |v: f64| {
        let v = v / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

/// WCAG 2 contrast ratio, `1.0..=21.0`, symmetric.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The worst contrast of `fg` against any of `bgs` (infinite when empty).
pub fn min_contrast(fg: Rgb, bgs: &[Rgb]) -> f64 {
    bgs.iter()
        .map(|&bg| contrast(fg, bg))
        .fold(f64::INFINITY, f64::min)
}

/// `fg`, rounded, with only its lightness moved until it reaches `min`
/// against every background in `bgs`. See [`ensure_contrast_by`].
pub fn ensure_contrast(fg: Rgb, bgs: &[Rgb], min: f64) -> Rgb {
    ensure_contrast_by(fg, min, |_| bgs.to_vec())
}

/// Like [`ensure_contrast`], for backgrounds that depend on the colour being
/// adjusted (a role's own soft tint). Only `fg`'s HSL lightness moves, toward
/// white or black, which keeps its hue and saturation (a plain blend toward
/// white would wash a red out to pink), by the smallest amount that passes. The preferred
/// direction is away from the backgrounds (toward white when `fg` is lighter
/// than their average); if that cannot reach `min` the other direction is
/// tried, and if neither can, the better endpoint is returned. A colour that
/// already passes is returned unchanged (rounded).
pub fn ensure_contrast_by(fg: Rgb, min: f64, bgs: impl Fn(Rgb) -> Vec<Rgb>) -> Rgb {
    let fg = fg.rounded();
    let worst = |c: Rgb| min_contrast(c, &bgs(c));
    if worst(fg) >= min {
        return fg;
    }
    let current = bgs(fg);
    let average =
        current.iter().map(|&c| relative_luminance(c)).sum::<f64>() / current.len() as f64;
    let directions = if relative_luminance(fg) >= average {
        [WHITE, BLACK]
    } else {
        [BLACK, WHITE]
    };
    for target in directions {
        let goal = if target == WHITE { 1.0 } else { 0.0 };
        if worst(target) < min {
            continue;
        }
        // Invariant: `hi` passes, `lo` does not. Bisection on the rounded
        // colour so the returned value is exactly the one that was checked.
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..32 {
            let mid = (lo + hi) / 2.0;
            if worst(with_lightness(fg, goal, mid)) >= min {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        return with_lightness(fg, goal, hi);
    }
    if worst(WHITE) >= worst(BLACK) {
        WHITE
    } else {
        BLACK
    }
}

/// HSL hue (0..6 sextants), saturation and lightness, each channel `0..=1`.
fn to_hsl(c: Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (c.0 / 255.0, c.1 / 255.0, c.2 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Rgb {
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u8 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = l - chroma / 2.0;
    Rgb((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

/// `c` with its HSL lightness moved fraction `t` of the way to `goal`
/// (0 = black, 1 = white), hue and saturation kept; rounded.
fn with_lightness(c: Rgb, goal: f64, t: f64) -> Rgb {
    let (h, s, l) = to_hsl(c);
    from_hsl(h, s, l + (goal - l) * t).rounded()
}

/// Whichever candidate contrasts most with `fill`.
pub fn best_on(fill: Rgb, candidates: &[Rgb]) -> Rgb {
    candidates
        .iter()
        .copied()
        .max_by(|&a, &b| contrast(a, fill).total_cmp(&contrast(b, fill)))
        .unwrap_or(BLACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(value: &str) -> Rgb {
        Rgb::parse(value).unwrap().0
    }

    #[test]
    fn parses_and_formats_hex() {
        assert_eq!(Rgb::parse("#0a0b0c"), Some((Rgb(10.0, 11.0, 12.0), 1.0)));
        let (rgb, alpha) = Rgb::parse("#ff000080").unwrap();
        assert_eq!(rgb, Rgb(255.0, 0.0, 0.0));
        assert!((alpha - 128.0 / 255.0).abs() < 1e-12);
        for bad in ["", "#", "#fff", "#12345", "#gggggg", "123456"] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
        assert_eq!(Rgb(1.4, 254.6, 300.0).hex(), "#01ffff");
    }

    #[test]
    fn known_wcag_pairs() {
        assert!((contrast(BLACK, WHITE) - 21.0).abs() < 1e-9);
        assert!((contrast(WHITE, WHITE) - 1.0).abs() < 1e-12);
        let c777 = contrast(hex("#777777"), WHITE);
        assert!((c777 - 4.48).abs() < 0.01, "{c777}");
        assert!(contrast(hex("#767676"), WHITE) >= 4.5);
        assert_eq!(
            contrast(hex("#123456"), WHITE),
            contrast(WHITE, hex("#123456"))
        );
    }

    #[test]
    fn mix_and_over_composite_like_css() {
        assert_eq!(mix(BLACK, WHITE, 0.0), BLACK);
        assert_eq!(mix(BLACK, WHITE, 1.0), WHITE);
        assert_eq!(mix(BLACK, WHITE, 0.5), Rgb(127.5, 127.5, 127.5));
        // rgba(255, 0, 0, .25) over white
        assert_eq!(
            over(Rgb(255.0, 0.0, 0.0), 0.25, WHITE),
            Rgb(255.0, 191.25, 191.25)
        );
    }

    #[test]
    fn blending_toward_white_raises_contrast_on_dark_monotonically() {
        let fg = hex("#5a4a8a");
        let bg = hex("#1e1e2e");
        let mut previous = 0.0;
        for step in 0..=20 {
            let ratio = contrast(mix(fg, WHITE, f64::from(step) / 20.0), bg);
            assert!(ratio >= previous);
            previous = ratio;
        }
    }

    #[test]
    fn ensure_contrast_returns_passing_input_unchanged() {
        let fg = hex("#202020");
        assert_eq!(ensure_contrast(fg, &[WHITE, hex("#eeeeee")], 4.5), fg);
        assert_eq!(ensure_contrast(fg, &[], 4.5), fg);
    }

    #[test]
    fn ensure_contrast_moves_lightness_just_enough() {
        // Too light on white: darkened, and only barely past the floor.
        let out = ensure_contrast(hex("#777777"), &[WHITE], 4.5);
        assert!(contrast(out, WHITE) >= 4.5);
        assert!(contrast(out, WHITE) < 4.7);
        assert!(out.0 == out.1 && out.1 == out.2, "a grey stays grey");
        // Too dark on several dark backgrounds: lightened, toward white.
        let bgs = [hex("#282a36"), hex("#44475a"), hex("#21222c")];
        let out = ensure_contrast(hex("#6272a4"), &bgs, 4.5);
        assert!(min_contrast(out, &bgs) >= 4.5);
        assert!(relative_luminance(out) > relative_luminance(hex("#6272a4")));
        // Hue is kept: blue stays the dominant channel.
        assert!(out.2 > out.0 && out.2 > out.1);
    }

    #[test]
    fn ensure_contrast_falls_back_to_the_reachable_direction() {
        // Lighter than a mid background but white cannot reach 7:1 on it.
        let bg = hex("#808080");
        let out = ensure_contrast(hex("#999999"), &[bg], 5.0);
        assert!(contrast(out, bg) >= 5.0);
        assert!(relative_luminance(out) < relative_luminance(bg));
        // Unreachable either way: the better endpoint, and it terminates.
        let out = ensure_contrast(hex("#999999"), &[bg], 15.0);
        assert_eq!(out, BLACK);
    }

    #[test]
    fn ensure_contrast_by_tracks_a_dependent_background() {
        let surface = hex("#fbf1c7");
        let tint = |c: Rgb| vec![surface, over(c, 32.0 / 255.0, surface)];
        let out = ensure_contrast_by(hex("#d79921"), 4.5, tint);
        assert!(min_contrast(out, &tint(out)) >= 4.5);
    }

    #[test]
    fn best_on_picks_the_strongest_candidate() {
        assert_eq!(best_on(hex("#ffe08a"), &[WHITE, BLACK]), BLACK);
        assert_eq!(best_on(hex("#1d4ed8"), &[WHITE, BLACK]), WHITE);
    }

    #[test]
    fn hsl_round_trips_and_lightness_keeps_hue() {
        for value in [
            "#fb4934", "#83a598", "#6272a4", "#000000", "#ffffff", "#b8bb26",
        ] {
            let c = hex(value);
            let (h, s, l) = to_hsl(c);
            assert_eq!(from_hsl(h, s, l).hex(), value);
        }
        let red = hex("#fb4934");
        let lighter = ensure_contrast(red, &[hex("#3c3836")], 6.0);
        let ((h0, s0, _), (h1, s1, _)) = (to_hsl(red), to_hsl(lighter));
        assert!(
            (h0 - h1).abs() < 0.02 && (s0 - s1).abs() < 0.03,
            "{lighter:?}"
        );
        assert_eq!(with_lightness(red, 1.0, 1.0), WHITE);
        assert_eq!(with_lightness(red, 0.0, 1.0), BLACK);
    }
}
