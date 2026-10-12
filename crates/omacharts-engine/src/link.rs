//! Link modes: what a chart does when a symbol is picked in the watchlist.
//!
//! The choice belongs on the chart, not on the list. Split a chart and click a
//! symbol, and the new chart should take it while the old one keeps what it
//! had — two actions, and it only works while the click goes to the focused
//! chart. A chart that should track the list has to be able to say so without
//! taking that away from every other chart, and a chart holding context (SPY
//! beside a stock) has to be able to refuse the click outright.
//!
//! So each chart has one of three modes, and every list drives the same
//! charts: the Follow ones, plus the focused chart when it is a Focus chart.

use std::fmt;

use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::theme::Theme;

/// What a chart does when a symbol is picked in the rail.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum LinkMode {
    /// Takes the rail's click while it is the focused chart. A symbol typed
    /// on it moves only itself.
    #[default]
    Focus,
    /// Takes every rail click, and a symbol typed on it moves every other
    /// Follow chart.
    Follow,
    /// Takes nothing from the rail.
    Pinned,
}

/// Every mode, in the order a picker offers them.
pub const MODES: [LinkMode; 3] = [LinkMode::Focus, LinkMode::Follow, LinkMode::Pinned];

impl LinkMode {
    /// What a chart written down with the nine link groups becomes.
    ///
    /// `group` is the chart's group, 0 for none, and `driven` the group the
    /// watchlist beside it drove. A chart in that group moved on every click
    /// in the list, which is what Follow does. A chart in any other group
    /// shared its symbol with charts the list never moved, a pool that no
    /// longer exists, so it goes back to the default: Follow would put it
    /// under a list that never drove it, and Pinned would have it refuse
    /// clicks nobody asked it to refuse. An unlinked chart is the default
    /// already.
    pub fn from_group(group: u8, driven: Option<u8>) -> LinkMode {
        match group {
            0 => LinkMode::Focus,
            group if Some(group) == driven => LinkMode::Follow,
            _ => LinkMode::Focus,
        }
    }

    /// What pinning or unpinning a chart makes it, which is what Ctrl+L does.
    ///
    /// A pinned chart goes back to the default, and any other is pinned. Never
    /// Follow: a chart made Follow leads, and every other Follow chart moves
    /// to its symbol, so a key that could land there would move charts nobody
    /// pressed it on. A Follow chart pinned leaves the Follow set quietly, the
    /// way leaving Follow always does.
    pub fn pin_toggled(self) -> LinkMode {
        match self {
            LinkMode::Pinned => LinkMode::Focus,
            LinkMode::Focus | LinkMode::Follow => LinkMode::Pinned,
        }
    }

    /// What the chart's link icon is painted in: nothing of its own for the
    /// default, so it dims with the legend, and the accent for a mode somebody
    /// chose, so a chart that moves on every click, or on none, stands out
    /// across four charts.
    pub fn colour(self, theme: &Theme) -> Option<String> {
        match self {
            LinkMode::Focus => None,
            LinkMode::Follow | LinkMode::Pinned => Some(theme.ui.accent.clone()),
        }
    }

    /// How a command spells it, and how it is written down.
    pub fn key(self) -> &'static str {
        match self {
            LinkMode::Focus => "focus",
            LinkMode::Follow => "follow",
            LinkMode::Pinned => "pinned",
        }
    }

    pub fn from_key(key: &str) -> Option<LinkMode> {
        MODES.into_iter().find(|mode| mode.key().eq_ignore_ascii_case(key))
    }

    /// What a menu row calls it.
    pub fn label(self) -> &'static str {
        match self {
            LinkMode::Focus => "Focus",
            LinkMode::Follow => "Follow",
            LinkMode::Pinned => "Pinned",
        }
    }

    /// What it does, for the row under its label and the chain's tooltip.
    pub fn blurb(self) -> &'static str {
        match self {
            LinkMode::Focus => "Follows watchlist while focused",
            LinkMode::Follow => "Always follows watchlist",
            LinkMode::Pinned => "Never follows watchlist",
        }
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// Written as its key.
impl Serialize for LinkMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

/// Read from its key, and anything else is Focus.
///
/// Lenient rather than fallible on purpose: this is read out of the stored
/// arrangement, and a mode from a newer version, or a hand-edited one, should
/// cost a chart its mode, not cost the user their workspace.
impl<'de> Deserialize<'de> for LinkMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<LinkMode, D::Error> {
        deserializer.deserialize_any(LinkModeVisitor)
    }
}

struct LinkModeVisitor;

impl Visitor<'_> for LinkModeVisitor {
    type Value = LinkMode;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("focus, follow or pinned")
    }

    fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<LinkMode, E> {
        Ok(LinkMode::from_key(key).unwrap_or_default())
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<LinkMode, E> {
        Ok(LinkMode::default())
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<LinkMode, E> {
        Ok(LinkMode::default())
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<LinkMode, E> {
        Ok(LinkMode::default())
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<LinkMode, E> {
        Ok(LinkMode::default())
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<LinkMode, E> {
        Ok(LinkMode::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_survives_being_written_down() {
        for mode in MODES {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.key()));
            assert_eq!(serde_json::from_str::<LinkMode>(&json).unwrap(), mode, "{json}");
            assert_eq!(LinkMode::from_key(mode.key()), Some(mode));
        }
    }

    #[test]
    fn a_mode_this_version_does_not_know_is_focus_and_nothing_worse() {
        // A newer version's mode, or a hand-edited one. Losing the mode is
        // recoverable in one click; refusing to parse would take the whole
        // layout down with it.
        for json in ["\"sideways\"", "3", "true", "null", "-1", "1.5"] {
            assert_eq!(serde_json::from_str::<LinkMode>(json).unwrap(), LinkMode::Focus, "{json}");
        }
    }

    #[test]
    fn pinning_from_the_keyboard_never_lands_on_follow() {
        // Landing on Follow makes the chart lead and moves every other Follow
        // chart, which is the jump a pin key must not cause.
        for (from, to) in [
            (LinkMode::Focus, LinkMode::Pinned),
            (LinkMode::Pinned, LinkMode::Focus),
            (LinkMode::Follow, LinkMode::Pinned),
        ] {
            assert_eq!(from.pin_toggled(), to, "{from:?}");
        }
    }

    #[test]
    fn a_chart_from_the_nine_groups_keeps_what_a_click_did_to_it() {
        // In the group its list drove: every click moved it.
        assert_eq!(LinkMode::from_group(1, Some(1)), LinkMode::Follow);
        assert_eq!(LinkMode::from_group(3, Some(3)), LinkMode::Follow);
        // In another group, or none: only while it was the focused chart.
        assert_eq!(LinkMode::from_group(2, Some(1)), LinkMode::Focus);
        assert_eq!(LinkMode::from_group(1, None), LinkMode::Focus);
        assert_eq!(LinkMode::from_group(0, Some(1)), LinkMode::Focus);
        assert_eq!(LinkMode::from_group(0, None), LinkMode::Focus);
    }
}
