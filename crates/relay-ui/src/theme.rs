//! Colour tokens from docs/design.md; one place so light and dark stay in step.

use gpui::{linear_color_stop, linear_gradient, rgb, rgba, Background, Hsla, WindowAppearance};

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

/// Curated tab-group colours. Every swatch is tested for WCAG AA contrast:
/// `dot` against `surface` (>= 3:1) and `text`/`muted` on alpha-blended `tint`
/// over `surface` (>= 4.5:1) in both light and dark themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupColor {
    Blue,
    Purple,
    Teal,
    Green,
    Orange,
    Pink,
}

impl GroupColor {
    pub const ALL: [GroupColor; 6] = [Self::Blue, Self::Purple, Self::Teal, Self::Green, Self::Orange, Self::Pink];

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "blue" => Some(Self::Blue),
            "purple" => Some(Self::Purple),
            "teal" => Some(Self::Teal),
            "green" => Some(Self::Green),
            "orange" => Some(Self::Orange),
            "pink" => Some(Self::Pink),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Teal => "teal",
            Self::Green => "green",
            Self::Orange => "orange",
            Self::Pink => "pink",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Teal => "Teal",
            Self::Green => "Green",
            Self::Orange => "Orange",
            Self::Pink => "Pink",
        }
    }

    /// Deterministic fallback colour for a group without an explicit choice.
    pub fn for_group_name(name: &str) -> Self {
        let hash = name.bytes().fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize));
        Self::ALL[hash % Self::ALL.len()]
    }

    pub fn dot(self, dark: bool) -> Hsla {
        let hex = match (self, dark) {
            (Self::Blue, true) => 0x5b8cff,
            (Self::Blue, false) => 0x2f6bff,
            (Self::Purple, true) => 0xb07cff,
            (Self::Purple, false) => 0x7a3cff,
            (Self::Teal, true) => 0x3bc9db,
            (Self::Teal, false) => 0x0c8599,
            (Self::Green, true) => 0x4ecb8d,
            (Self::Green, false) => 0x1f7a4d,
            (Self::Orange, true) => 0xffb84d,
            (Self::Orange, false) => 0xa35f00,
            (Self::Pink, true) => 0xf783ac,
            (Self::Pink, false) => 0xc2255c,
        };
        rgb(hex).into()
    }

    /// Subtle heading background tint over `surface` (keeps AA text contrast).
    pub fn tint(self, dark: bool) -> Hsla {
        let hex = match (self, dark) {
            (Self::Blue, true) => 0x5b8cff1a,
            (Self::Blue, false) => 0x2f6bff14,
            (Self::Purple, true) => 0xb07cff1a,
            (Self::Purple, false) => 0x7a3cff14,
            (Self::Teal, true) => 0x3bc9db18,
            (Self::Teal, false) => 0x0c859914,
            (Self::Green, true) => 0x4ecb8d18,
            (Self::Green, false) => 0x1f7a4d14,
            (Self::Orange, true) => 0xffb84d18,
            (Self::Orange, false) => 0xa35f0014,
            (Self::Pink, true) => 0xf783ac1a,
            (Self::Pink, false) => 0xc2255c14,
        };
        rgba(hex).into()
    }
}

/// Blank-tab / start-page background: default quiet page, solid/gradient
/// presets (with light/dark variants checked for WCAG AA contrast), or a
/// validated local image file. Remote URLs are never accepted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StartBackground {
    #[default]
    Default,
    Slate,
    Sage,
    Sand,
    Dusk,
    Forest,
    Warmth,
    Image(String),
}

impl StartBackground {
    pub const PRESETS: [StartBackground; 7] = [
        Self::Default,
        Self::Slate,
        Self::Sage,
        Self::Sand,
        Self::Dusk,
        Self::Forest,
        Self::Warmth,
    ];

    pub fn parse(raw: Option<&str>) -> Self {
        let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::Default;
        };
        match raw.to_ascii_lowercase().as_str() {
            "default" | "none" => Self::Default,
            "slate" => Self::Slate,
            "sage" => Self::Sage,
            "sand" => Self::Sand,
            "dusk" => Self::Dusk,
            "forest" => Self::Forest,
            "warmth" => Self::Warmth,
            _ => {
                if let Some(path) = raw.strip_prefix("file:") {
                    if Self::is_safe_local_path(path) {
                        return Self::Image(path.to_string());
                    }
                }
                Self::Default
            }
        }
    }

    pub fn is_safe_local_path(path: &str) -> bool {
        let p = path.trim();
        if p.is_empty() || p.len() > 2048 || p.chars().any(char::is_control) {
            return false;
        }
        let lower = p.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("data:") || lower.starts_with("javascript:") {
            return false;
        }
        std::path::Path::new(p).is_absolute()
    }

    pub fn to_setting(&self) -> String {
        match self {
            Self::Default => "default".into(),
            Self::Slate => "slate".into(),
            Self::Sage => "sage".into(),
            Self::Sand => "sand".into(),
            Self::Dusk => "dusk".into(),
            Self::Forest => "forest".into(),
            Self::Warmth => "warmth".into(),
            Self::Image(path) => format!("file:{path}"),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Slate => "Slate",
            Self::Sage => "Sage",
            Self::Sand => "Sand",
            Self::Dusk => "Dusk",
            Self::Forest => "Forest",
            Self::Warmth => "Warmth",
            Self::Image(_) => "Image",
        }
    }

    /// Start and end colours for solid/gradient presets (equal for solids).
    pub fn stops(&self, dark: bool) -> Option<(Hsla, Hsla)> {
        let (a, b) = match (self, dark) {
            (Self::Slate, true) => (0x181d26, 0x181d26),
            (Self::Slate, false) => (0xf2f5fa, 0xf2f5fa),
            (Self::Sage, true) => (0x17201c, 0x17201c),
            (Self::Sage, false) => (0xf1f7f3, 0xf1f7f3),
            (Self::Sand, true) => (0x211d19, 0x211d19),
            (Self::Sand, false) => (0xf8f4ee, 0xf8f4ee),
            (Self::Dusk, true) => (0x171b28, 0x221c2c),
            (Self::Dusk, false) => (0xf1f5fc, 0xf6f1fa),
            (Self::Forest, true) => (0x15201c, 0x1a222b),
            (Self::Forest, false) => (0xf1f8f4, 0xf1f6fa),
            (Self::Warmth, true) => (0x231d19, 0x1b1d24),
            (Self::Warmth, false) => (0xfaf4ec, 0xf4f2f8),
            (Self::Default | Self::Image(_), _) => return None,
        };
        Some((rgb(a).into(), rgb(b).into()))
    }

    pub fn background(&self, dark: bool, fallback_page: Hsla) -> Background {
        match self.stops(dark) {
            Some((a, b)) if a == b => Background::from(a),
            Some((a, b)) => linear_gradient(145.0, linear_color_stop(a, 0.0), linear_color_stop(b, 1.0)),
            None => Background::from(fallback_page),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Hsla,
    pub surface: Hsla,
    /// Cards and overlays: one step above `surface`, separated by a shadow
    /// rather than a 1 px outline.
    pub raised: Hsla,
    /// Inputs and wells: one step below `surface`, so a field reads as a
    /// recess without a border.
    pub well: Hsla,
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
            raised: rgb(0x292b31).into(),
            well: rgb(0x151619).into(),
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
            raised: rgb(0xffffff).into(),
            well: rgb(0xededf0).into(),
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
            assert_eq!(GroupColor::parse(value), None, "{value}");
        }
    }

    fn blend_over(fg: Hsla, bg: Hsla) -> Hsla {
        let f = fg.to_rgb();
        let b = bg.to_rgb();
        let a = fg.a.clamp(0.0, 1.0);
        gpui::Rgba {
            r: f.r * a + b.r * (1.0 - a),
            g: f.g * a + b.g * (1.0 - a),
            b: f.b * a + b.b * (1.0 - a),
            a: 1.0,
        }
        .into()
    }

    #[test]
    fn every_group_color_meets_wcag_aa_on_surface() {
        for (mode, dark, theme) in [("dark", true, Theme::dark()), ("light", false, Theme::light())] {
            for color in GroupColor::ALL {
                assert_eq!(GroupColor::parse(color.as_str()), Some(color));
                let dot_ratio = contrast_ratio(color.dot(dark), theme.surface);
                assert!(dot_ratio >= 3.0, "{mode} {color:?} dot contrast {dot_ratio:.2}:1 < 3.0");
                let bg = blend_over(color.tint(dark), theme.surface);
                let text_ratio = contrast_ratio(theme.text, bg);
                let muted_ratio = contrast_ratio(theme.muted, bg);
                assert!(text_ratio >= 4.5, "{mode} {color:?} text/tint contrast {text_ratio:.2}:1 < 4.5");
                assert!(muted_ratio >= 4.5, "{mode} {color:?} muted/tint contrast {muted_ratio:.2}:1 < 4.5");
            }
        }
    }

    #[test]
    fn every_start_background_preset_meets_wcag_aa() {
        for (mode, dark, theme) in [("dark", true, Theme::dark()), ("light", false, Theme::light())] {
            for preset in StartBackground::PRESETS {
                assert_eq!(StartBackground::parse(Some(&preset.to_setting())), preset);
                if let Some((start, end)) = preset.stops(dark) {
                    for (stop_name, bg) in [("start", start), ("end", end)] {
                        let text_ratio = contrast_ratio(theme.text, bg);
                        let muted_ratio = contrast_ratio(theme.muted, bg);
                        assert!(text_ratio >= 4.5, "{mode} {preset:?} {stop_name} text contrast {text_ratio:.2}:1 < 4.5");
                        assert!(muted_ratio >= 4.5, "{mode} {preset:?} {stop_name} muted contrast {muted_ratio:.2}:1 < 4.5");
                    }
                }
            }
        }
        // Remote URLs and relative paths are never accepted as start-page backgrounds.
        for bad in ["https://evil.example/bg.png", "file:https://evil.example/bg.png", "file:data:image/png;base64,AA==", "file:relative.png", "neon"] {
            assert_eq!(StartBackground::parse(Some(bad)), StartBackground::Default, "{bad}");
        }
        assert_eq!(
            StartBackground::parse(Some("file:/tmp/wallpaper.png")),
            StartBackground::Image("/tmp/wallpaper.png".into())
        );
    }
}
