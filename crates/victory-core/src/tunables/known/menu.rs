//! The switches the F1 menu lists (victory-gpu `host/menu.rs`).
//!
//! Not declared: `VICTORY_LEAVE_OUT` (a list of shader hashes, free text) and
//! the launcher's own `VICTORY_MENU_*` choices (the launcher reads those from
//! its settings file and turns them into the switches below and elsewhere).

use crate::{tunable, tunable_list};

tunable!(pub FPS_LINE: bool = true, "VICTORY_FPS_LINE",
    "Frame rate line at the top of the picture (on unless the variable is 0)", live = Safe);
tunable!(pub PANEL: bool = false, "VICTORY_PANEL",
    "Stats panel (frame rate, GPU and memory numbers); F3 shows and hides it", live = Safe);
tunable!(pub FULLSCREEN: bool = false, "VICTORY_FULLSCREEN",
    "Borderless fullscreen window", live = Restart);
tunable!(pub MOUSE_CAPTURE: bool = false, "VICTORY_MOUSE_CAPTURE",
    "Hide and hold the mouse cursor while the window has the focus", live = Restart);
tunable!(pub IME_ENGLISH: bool = false, "VICTORY_IME_ENGLISH",
    "Force the English keyboard layout for the game window", live = Restart);
tunable!(pub DLSS_PRESET: enum = "default", ["default", "e", "f", "j", "k", "l", "m"], "VICTORY_DLSS_PRESET",
    "DLSS model preset", live = Restart);
tunable!(pub AMBIENT: f64 = 0.0, 0.0..=100.0, "VICTORY_AMBIENT",
    "Light estimate: a share of the light the visible surfaces send out (0 is off)", live = Restart);
tunable!(pub AMBIENT_AIR: f64 = 0.0, 0.0..=100.0, "VICTORY_AMBIENT_AIR",
    "Most the air scatters, as a multiple of the light around (0 keeps the title's own)", live = Restart);
tunable!(pub BVH_BUILDER: bool = false, "VICTORY_BVH_BUILDER",
    "Run the title's own ray tracing structure builder (slow, broken)", live = Restart);
tunable!(pub WATER_LIGHT: enum = "on", ["on", "0"], "VICTORY_WATER_LIGHT",
    "0 leaves out the water's two tiled light programs on 32-lane hosts", live = Restart);
tunable!(pub EARLY_DEPTH: bool = false, "VICTORY_EARLY_DEPTH",
    "Early depth-stencil test for pixel programs whose console code tests depth first", live = Restart);
tunable!(pub WC_NO_BACK: bool = false, "VICTORY_WC_NO_BACK",
    "Skip big copy-backs of write-combined memory", live = Restart);
tunable!(pub SOFT_VIEWS: enum = "0", ["0", "1", "pool", "copies"], "VICTORY_SOFT_VIEWS",
    "Soft views of memory sections (no cut freezes): 1 pool and copies, pool, copies", live = Restart);
tunable!(pub TEXTURE_NO_WAIT: bool = false, "VICTORY_TEXTURE_NO_WAIT",
    "Texture uploads do not wait for the card", live = Restart);
tunable!(pub RELAY: enum = "2", ["2", "1", "0"], "VICTORY_RELAY",
    "Relay stage (speed, triangles): 2 and 1 on, 0 off", live = Restart);
tunable!(pub CARD_STEPS: enum = "0", ["0", "1", "2"], "VICTORY_CARD_STEPS",
    "Card steps stage", live = Restart);
tunable!(pub INDIRECT_BATCH: enum = "1", ["1", "0", "2"], "VICTORY_INDIRECT_BATCH",
    "Indirect draw batching stage", live = Restart);
tunable!(pub CARD_BUFFERS_MIN_KB: i64 = 0, 0..=100000000, "VICTORY_CARD_BUFFERS_MIN_KB",
    "Least size in KiB of a buffer copied onto the card (0 keeps the built-in minimum)", live = Restart);
tunable!(pub PORT_CONSTANTS: bool = false, "VICTORY_PORT_CONSTANTS",
    "Constants heap in place", live = Restart);

tunable_list!(pub(super) fn register: FPS_LINE, PANEL, FULLSCREEN, MOUSE_CAPTURE, IME_ENGLISH, DLSS_PRESET, AMBIENT, AMBIENT_AIR, BVH_BUILDER, WATER_LIGHT, EARLY_DEPTH, WC_NO_BACK, SOFT_VIEWS, TEXTURE_NO_WAIT, RELAY, CARD_STEPS, INDIRECT_BATCH, CARD_BUFFERS_MIN_KB, PORT_CONSTANTS);
