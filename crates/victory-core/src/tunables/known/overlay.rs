//! The look of the performance overlay (victory-gpu `host/panel.rs`; the Overlay page of the menu).
//! Whether it is shown is `PANEL` (`known/menu.rs`); F2 flips it.

use crate::{tunable, tunable_list};

tunable!(pub STATS_PANEL_CORNER: enum = "top-left", ["top-left", "top-right", "bottom-left", "bottom-right"], "VICTORY_STATS_PANEL_CORNER",
    "Corner of the picture the overlay sits in", live = Safe);
tunable!(pub STATS_PANEL_SIZE: enum = "normal", ["small", "normal", "large"], "VICTORY_STATS_PANEL_SIZE",
    "Size of the overlay's text: small, normal or large", live = Safe);
tunable!(pub STATS_PANEL_LINES: enum = "full", ["full", "fps"], "VICTORY_STATS_PANEL_LINES",
    "Lines of the overlay: full (all the numbers) or fps (frame rate and frame time only)", live = Safe);

tunable_list!(pub(super) fn register: STATS_PANEL_CORNER, STATS_PANEL_SIZE, STATS_PANEL_LINES);
