//! Colour tokens from docs/design.md; one place so light and dark stay in step.

use gpui::{rgb, rgba, Hsla, WindowAppearance};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "system" | "auto" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccentChoice {
    #[default]
    Blue,
    Purple,
    Green,
    Orange,
}

impl AccentChoice {
    pub const ALL: [AccentChoice; 4] = [Self::Blue, Self::Purple, Self::Green, Self::Orange];

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "blue" => Some(Self::Blue),
            "purple" => Some(Self::Purple),
            "green" => Some(Self::Green),
            "orange" => Some(Self::Orange),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Green => "green",
            Self::Orange => "orange",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Green => "Green",
            Self::Orange => "Orange",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Blue => Self::Purple,
            Self::Purple => Self::Green,
            Self::Green => Self::Orange,
            Self::Orange => Self::Blue,
        }
    }

    pub(crate) fn color(self, dark: bool) -> (Hsla, Hsla) {
        let (accent, overlay) = match (self, dark) {
            (Self::Blue, true) => (0x5b8cff, 0x5b8cff33),
            (Self::Blue, false) => (0x2f6bff, 0x2f6bff24),
            (Self::Purple, true) => (0xb07cff, 0xb07cff38),
            (Self::Purple, false) => (0x7a3cff, 0x7a3cff24),
            (Self::Green, true) => (0x4ecb8d, 0x4ecb8d38),
            (Self::Green, false) => (0x1f7a4d, 0x1f7a4d24),
            (Self::Orange, true) => (0xffb84d, 0xffb84d38),
            (Self::Orange, false) => (0xa35f00, 0xa35f0029),
        };
        (rgb(accent).into(), rgba(overlay).into())
    }
}

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

impl ThemeChoice {
    pub fn is_dark(self, appearance: WindowAppearance) -> bool {
        match self {
            ThemeChoice::System => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
        }
    }
}

impl Theme {
    pub fn for_choice(appearance: WindowAppearance, choice: ThemeChoice, accent: AccentChoice) -> Self {
        let dark = choice.is_dark(appearance);
        let mut theme = if dark { Self::dark() } else { Self::light() };
        let (color, active) = accent.color(dark);
        theme.accent = color;
        theme.active = active;
        theme
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
            error: rgb(0xc92f2f).into(),
            warn: rgb(0x8a5300).into(),
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
    fn every_named_theme_and_accent_pair_meets_wcag_aa() {
        assert_wcag_aa("dark", Theme::dark());
        assert_wcag_aa("light", Theme::light());
        // The MCP accepts only these named tokens, so covering every choice
        // pair here is the runtime guarantee for an agent-planned theme.
        for choice in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
            for accent in AccentChoice::ALL {
                for appearance in [WindowAppearance::Light, WindowAppearance::Dark] {
                    let theme = Theme::for_choice(appearance, choice, accent);
                    assert_wcag_aa(&format!("{appearance:?}/{choice:?}/{accent:?}"), theme);
                }
            }
        }
    }

    #[test]
    fn choices_round_trip_and_system_follows_appearance() {
        for choice in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
            assert_eq!(ThemeChoice::parse(choice.as_str()), Some(choice));
        }
        for accent in AccentChoice::ALL {
            assert_eq!(AccentChoice::parse(accent.as_str()), Some(accent));
        }
        assert_eq!(ThemeChoice::Light.next(), ThemeChoice::Dark);
        assert_eq!(AccentChoice::Orange.next(), AccentChoice::Blue);
        assert_eq!(Theme::for_choice(WindowAppearance::Dark, ThemeChoice::System, AccentChoice::Blue).background, Theme::dark().background);
        assert_eq!(Theme::for_choice(WindowAppearance::Light, ThemeChoice::Dark, AccentChoice::Blue).background, Theme::dark().background);
        assert!(ThemeChoice::Dark.is_dark(WindowAppearance::Light));
        assert!(!ThemeChoice::Light.is_dark(WindowAppearance::Dark));
        assert!(ThemeChoice::System.is_dark(WindowAppearance::Dark));
        assert!(ThemeChoice::System.is_dark(WindowAppearance::VibrantDark));
        assert!(!ThemeChoice::System.is_dark(WindowAppearance::Light));
    }

    #[test]
    fn unlisted_theme_tokens_are_rejected() {
        // Raw colours and unknown names never reach a palette: the MCP rejects
        // the same set, and a future palette entry must pass the contrast test
        // above before it can be named here.
        for value in ["neon", "#ff00ff", "rgb(255,0,255)", "system,light"] {
            assert_eq!(ThemeChoice::parse(value), None, "{value}");
        }
        for value in ["magenta", "#ff00ff", "orange,blue"] {
            assert_eq!(AccentChoice::parse(value), None, "{value}");
        }
    }
}
