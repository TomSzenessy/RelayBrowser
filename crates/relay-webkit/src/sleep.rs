//! Aggressive tab sleeping: background tabs untouched for 15 minutes (not
//! active, not pinned, not playing audio, not agent-driven right now) are
//! snapshotted and their WKWebView destroyed. Pure policy; the macOS layer
//! performs the snapshot/destroy/restore.

use relay_bridge::{Hub, TabId};
use std::time::{Duration, Instant};

pub const DEFAULT_IDLE: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy)]
pub struct SleepPolicy {
    pub idle_after: Duration,
    /// Never sleep below this many awake tabs (cheap insurance for tiny sessions).
    pub keep_awake_min: usize,
}

impl Default for SleepPolicy {
    fn default() -> Self {
        Self { idle_after: DEFAULT_IDLE, keep_awake_min: 1 }
    }
}

/// What a sleeping tab keeps so it can wake instantly.
#[derive(Debug, Clone, PartialEq)]
pub struct SleptTab {
    pub url: String,
    pub scroll_x: f64,
    pub scroll_y: f64,
    /// JPEG of the last viewport, shown while WebKit reloads.
    pub snapshot_jpeg: Option<Vec<u8>>,
}

impl SleepPolicy {
    /// Tabs that should go to sleep now, oldest first.
    pub fn due(&self, hub: &Hub, visible: &[TabId], now: Instant) -> Vec<TabId> {
        let awake = hub.tabs().filter(|t| !t.sleeping).count();
        let mut due: Vec<_> = hub
            .tabs()
            .filter(|t| !t.sleeping && !t.pinned && !t.playing_audio)
            .filter(|t| Some(t.id) != hub.active() && !visible.contains(&t.id))
            .filter(|t| now.saturating_duration_since(t.last_active_at) >= self.idle_after)
            .map(|t| (t.last_active_at, t.id))
            .collect();
        due.sort();
        let can_sleep = awake.saturating_sub(self.keep_awake_min);
        due.into_iter().take(can_sleep).map(|(_, id)| id).collect()
    }
}

/// Wake-up script: restore scroll once the reloaded page is ready.
pub fn restore_scroll_script(s: &SleptTab) -> String {
    format!(
        "(function(){{var go=function(){{window.scrollTo({x},{y});}};if(document.readyState==='complete')go();else window.addEventListener('load',go,{{once:true}});}})()",
        x = s.scroll_x,
        y = s.scroll_y
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_bridge::{Owner, SharedHub};

    #[test]
    fn sleeps_only_idle_background_tabs() {
        let hub = SharedHub::new();
        let mut h = hub.lock();
        let a = h.create_tab("a", Owner::User);
        let b = h.create_tab("b", Owner::User);
        let c = h.create_tab("c", Owner::User);
        let d = h.create_tab("d", Owner::User);
        let e = h.create_tab("e", Owner::User);
        h.set_active(a);
        h.tab_mut(c).unwrap().pinned = true;
        h.tab_mut(d).unwrap().playing_audio = true;
        let p = SleepPolicy::default();
        let later = Instant::now() + Duration::from_secs(16 * 60);
        assert!(p.due(&h, &[], Instant::now()).is_empty(), "nothing idle yet");
        // a is active, c pinned, d audio, e visible in a split pane.
        assert_eq!(p.due(&h, &[e], later), vec![b]);
        h.tab_mut(b).unwrap().sleeping = true;
        assert!(p.due(&h, &[e], later).is_empty());
    }

    #[test]
    fn keeps_a_minimum_awake() {
        let hub = SharedHub::new();
        let mut h = hub.lock();
        let a = h.create_tab("a", Owner::User);
        let b = h.create_tab("b", Owner::User);
        let _ = (a, b);
        let p = SleepPolicy { idle_after: Duration::ZERO, keep_awake_min: 2 };
        assert!(p.due(&h, &[], Instant::now()).is_empty());
    }

    #[test]
    fn scroll_script_embeds_numbers_only() {
        let s = restore_scroll_script(&SleptTab { url: "u".into(), scroll_x: 0.0, scroll_y: 1200.5, snapshot_jpeg: None });
        assert!(s.contains("window.scrollTo(0,1200.5)"));
    }
}
