//! Where the tab strip sits and whether it is folded away.
//!
//! Search taught the useful shape: tabs live down one side by default, `⌘S`
//! folds the strip to a thin rail and `⇧⌘S` moves it. The geometry and the
//! settings encoding are pure data so they can be tested off-Mac; `app.rs`
//! only paints them.

/// The three edges a vertical or horizontal strip may occupy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    #[default]
    Left,
    Top,
    Right,
}

impl Placement {
    pub const ALL: [Placement; 3] = [Placement::Left, Placement::Top, Placement::Right];

    /// `⌘⇧S` order: left, top, right, and back to left.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Placement::Left => "left",
            Placement::Top => "top",
            Placement::Right => "right",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Placement::Left => "Left",
            Placement::Top => "Top",
            Placement::Right => "Right",
        }
    }

    /// Unknown or missing settings fall back to the default, never fail to
    /// start a window over a hand-edited profile.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" => Some(Placement::Left),
            "top" => Some(Placement::Top),
            "right" => Some(Placement::Right),
            _ => None,
        }
    }

    pub fn is_vertical(self) -> bool {
        self != Placement::Top
    }
}

/// Expanded strip width for left/right and height for top.
pub const VERTICAL_WIDTH: f32 = 220.0;
/// The folded rail keeps a visible, clickable edge to pull the strip back out.
pub const VERTICAL_FOLDED_WIDTH: f32 = 14.0;
pub const TOP_HEIGHT: f32 = 38.0;
pub const TOP_FOLDED_HEIGHT: f32 = 10.0;

/// The tab strip's persisted presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripState {
    pub placement: Placement,
    pub folded: bool,
}

impl Default for StripState {
    fn default() -> Self {
        Self { placement: Placement::default(), folded: false }
    }
}

impl StripState {
    /// Settings keys: `tabs_position` (left/top/right) and `tabs_folded`
    /// (`on`/`off`). Anything else keeps the safe default.
    pub fn from_settings(position: Option<&str>, folded: Option<&str>) -> Self {
        Self {
            placement: position.and_then(Placement::parse).unwrap_or_default(),
            folded: matches!(folded.map(str::trim), Some("on" | "On" | "ON" | "true" | "1")),
        }
    }

    pub fn position_setting(self) -> &'static str {
        self.placement.as_str()
    }

    pub fn folded_setting(self) -> &'static str {
        if self.folded { "on" } else { "off" }
    }

    pub fn toggled(self) -> Self {
        Self { folded: !self.folded, ..self }
    }

    pub fn rotated(self) -> Self {
        Self { placement: self.placement.next(), ..self }
    }

    /// A folded strip still reports a real width/height: the rail.
    pub fn width(self) -> f32 {
        if self.folded { VERTICAL_FOLDED_WIDTH } else { VERTICAL_WIDTH }
    }

    pub fn height(self) -> f32 {
        if self.folded { TOP_FOLDED_HEIGHT } else { TOP_HEIGHT }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_visits_every_edge_and_returns() {
        let mut p = Placement::Left;
        let mut seen = vec![p];
        for _ in 0..2 {
            p = p.next();
            seen.push(p);
        }
        assert_eq!(seen, vec![Placement::Left, Placement::Top, Placement::Right]);
        assert_eq!(p.next(), Placement::Left);
        for p in Placement::ALL {
            assert_eq!(Placement::parse(p.as_str()), Some(p));
            assert_eq!(p.is_vertical(), p != Placement::Top);
        }
    }

    #[test]
    fn unknown_settings_fall_back_instead_of_failing() {
        assert_eq!(StripState::from_settings(None, None), StripState::default());
        assert_eq!(StripState::from_settings(Some("sideways"), Some("maybe")), StripState::default());
        assert_eq!(StripState::from_settings(Some("TOP"), Some("on")), StripState { placement: Placement::Top, folded: true });
        assert_eq!(StripState::from_settings(Some("right"), Some("off")), StripState { placement: Placement::Right, folded: false });
    }

    #[test]
    fn settings_round_trip() {
        for placement in Placement::ALL {
            for folded in [false, true] {
                let state = StripState { placement, folded };
                let again = StripState::from_settings(Some(state.position_setting()), Some(state.folded_setting()));
                assert_eq!(again, state);
            }
        }
    }

    #[test]
    fn folding_and_rotating_are_independent() {
        let s = StripState::default();
        assert_eq!(s.toggled(), StripState { folded: true, ..s });
        assert_eq!(s.rotated(), StripState { placement: Placement::Top, ..s });
        assert_eq!(s.toggled().rotated(), StripState { placement: Placement::Top, folded: true });
        assert!(s.width() > s.toggled().width());
        assert!(s.height() > s.toggled().height());
    }
}
