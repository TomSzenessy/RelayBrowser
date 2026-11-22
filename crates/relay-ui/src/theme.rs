//! Colour tokens from docs/design.md; one place so light and dark stay in step.

use gpui::{rgb, rgba, Hsla, WindowAppearance};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Hsla,
    pub surface: Hsla,
    pub page: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub code: Hsla,
    pub accent: Hsla,
    pub active: Hsla,
    pub hover: Hsla,
    pub agent: Hsla,
    pub agent_bg: Hsla,
    pub error: Hsla,
    pub warn: Hsla,
    pub scrim: Hsla,
}

impl Theme {
    pub fn for_appearance(a: WindowAppearance) -> Self {
        match a {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: rgb(0x1b1c1f).into(),
            surface: rgb(0x232428).into(),
            page: rgb(0x1f2023).into(),
            border: rgb(0x34363b).into(),
            text: rgb(0xe6e6e9).into(),
            muted: rgb(0x9a9ca3).into(),
            code: rgb(0xc9d1ff).into(),
            accent: rgb(0x5b8cff).into(),
            active: rgba(0x5b8cff33).into(),
            hover: rgba(0xffffff12).into(),
            agent: rgb(0xb07cff).into(),
            agent_bg: rgba(0xb07cff2e).into(),
            error: rgb(0xff6b6b).into(),
            warn: rgb(0xffb84d).into(),
            scrim: rgba(0x0000004d).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            background: rgb(0xf6f6f7).into(),
            surface: rgb(0xffffff).into(),
            page: rgb(0xfbfbfc).into(),
            border: rgb(0xe2e2e6).into(),
            text: rgb(0x1c1c1f).into(),
            muted: rgb(0x6b6d75).into(),
            code: rgb(0x2a3a8f).into(),
            accent: rgb(0x2f6bff).into(),
            active: rgba(0x2f6bff24).into(),
            hover: rgba(0x0000000d).into(),
            agent: rgb(0x7a3cff).into(),
            agent_bg: rgba(0x7a3cff1f).into(),
            error: rgb(0xc42b2b).into(),
            warn: rgb(0x9a5c00).into(),
            scrim: rgba(0x00000026).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linearize(c: f32) -> f64 {
        let c = c.clamp(0.0, 1.0) as f64;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn relative_luminance(c: Hsla) -> f64 {
        let rgb = c.to_rgb();
        0.2126 * linearize(rgb.r) + 0.7152 * linearize(rgb.g) + 0.0722 * linearize(rgb.b)
    }

    fn contrast_ratio(a: Hsla, b: Hsla) -> f64 {
        let (l1, l2) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    fn assert_wcag_aa(name: &str, t: Theme) {
        for (pair, fg, bg, min) in [
            ("text/background", t.text, t.background, 4.5),
            ("text/surface", t.text, t.surface, 4.5),
            ("text/page", t.text, t.page, 4.5),
            ("muted/background", t.muted, t.background, 4.5),
            ("muted/surface", t.muted, t.surface, 4.5),
            ("code/surface", t.code, t.surface, 4.5),
            ("accent/surface", t.accent, t.surface, 3.0),
            // Warning and error labels are 12px text, so they need 4.5:1.
            ("error/surface", t.error, t.surface, 4.5),
            ("error/background", t.error, t.background, 4.5),
            ("warn/surface", t.warn, t.surface, 4.5),
            ("warn/background", t.warn, t.background, 4.5),
            ("agent/surface", t.agent, t.surface, 4.5),
        ] {
            let ratio = contrast_ratio(fg, bg);
            assert!(
                ratio >= min,
                "{name} {pair} contrast {ratio:.2}:1 is below WCAG AA {min:.1}:1"
            );
        }
    }

    #[test]
    fn dark_and_light_meet_wcag_aa_contrast() {
        assert_wcag_aa("dark", Theme::dark());
        assert_wcag_aa("light", Theme::light());
    }
}
