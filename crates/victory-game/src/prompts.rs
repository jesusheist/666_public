//! Keyboard key caps in place of the title's gamepad button pictures.
//!
//! The title draws a control's prompt as an icon of one UI movie, picked by
//! an icon number, and has icons for gamepad controls only. [`key_caps_with`]
//! redraws that movie: each icon becomes the key that the keyboard's stand-in
//! gamepad binds to the icon's control, in the bind table in force
//! (`VICTORY_BINDS`). [`hook`] hands the redrawn movie to the title. The
//! movie is recognised by its structure, and nothing of it is kept here.
//! The movie is drawn once, so prompts keep the layout (on foot or
//! driving) in force when it was drawn, and do not follow a device change;
//! the title's animated prompts keep their own pictures.
//!
//! By imjustprism (Marvisak/VIctory#3433), ported with the bind tables.

use std::collections::{HashMap, HashSet};

use pad::{
    PadControl, VK_BACK, VK_CAPITAL, VK_DOWN, VK_ESCAPE, VK_LBUTTON, VK_LCONTROL, VK_LEFT,
    VK_LMENU, VK_LSHIFT, VK_MBUTTON, VK_RBUTTON, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU,
    VK_RSHIFT, VK_SPACE, VK_TAB, VK_UP, VK_XBUTTON1, VK_XBUTTON2,
};
use victory_formats::gfx::{
    self, Change, Contour, Depth, Edge, Movie, Placement, Point, ShapeBody, Sprite,
};

#[cfg(windows)]
mod font;
pub mod pad;
#[cfg(windows)]
pub mod hook;

/// An offset from an image's base to an 8-byte pointer.
#[derive(Debug, Clone, Copy)]
pub struct PointerOffset(pub u32);

/// An offset from an image's base to a function's first instruction.
#[derive(Debug, Clone, Copy)]
pub struct CodeOffset(pub u32);

/// Where a build keeps what the hook needs.
#[derive(Debug, Clone, Copy)]
pub struct HookOffsets {
    /// The pointer through which the UI runtime calls the function that
    /// builds a movie's address from a request.
    pub movie_address_builder: PointerOffset,
    /// The function that assigns C text to a UI runtime string.
    pub string_assign: CodeOffset,
}

/// The icon numbers and the gamepad controls each one pictures: the
/// numbers in the class names of the icon movie's sprites (Series 1.0.2.1),
/// matched to the pictures the title's own icons show.
const ICONS: &[(u32, &[PadControl])] = {
    use PadControl as C;
    const LEFT: &[PadControl] = &[
        C::LeftStickUp,
        C::LeftStickLeft,
        C::LeftStickDown,
        C::LeftStickRight,
    ];
    const RIGHT: &[PadControl] = &[
        C::RightStickUp,
        C::RightStickLeft,
        C::RightStickDown,
        C::RightStickRight,
    ];
    const PAD: &[PadControl] = &[C::Up, C::Down, C::Left, C::Right];
    &[
        (4, &[C::Up]),
        (5, &[C::Down]),
        (6, &[C::Left]),
        (7, &[C::Right]),
        (8, PAD),
        (9, PAD),
        (10, &[C::Up, C::Down]),
        (11, &[C::Left, C::Right]),
        (12, &[C::LeftStickUp]),
        (13, &[C::LeftStickDown]),
        (14, &[C::LeftStickLeft]),
        (15, &[C::LeftStickRight]),
        (16, &[C::LeftStick]),
        (17, LEFT),
        (18, &[C::LeftStickUp, C::LeftStickDown]),
        (19, &[C::LeftStickLeft, C::LeftStickRight]),
        (20, LEFT),
        (21, &[C::RightStickUp]),
        (22, &[C::RightStickDown]),
        (23, &[C::RightStickLeft]),
        (24, &[C::RightStickRight]),
        (25, &[C::RightStick]),
        (26, RIGHT),
        (27, &[C::RightStickUp, C::RightStickDown]),
        (28, &[C::RightStickLeft, C::RightStickRight]),
        (29, RIGHT),
        (30, &[C::A]),
        (31, &[C::B]),
        (32, &[C::X]),
        (33, &[C::Y]),
        (34, &[C::LeftShoulder]),
        (35, &[C::LeftTrigger]),
        (36, &[C::RightShoulder]),
        (37, &[C::RightTrigger]),
        (38, &[C::Menu]),
        (39, &[C::View]),
        (58, LEFT),
        (59, RIGHT),
    ]
};

/// A movie with fewer icons than this is not the icon movie.
const FEWEST_ICONS: usize = 20;
/// Controls whose icons the icon movie has besides.
const FACE_BUTTONS: [PadControl; 4] = [PadControl::A, PadControl::B, PadControl::X, PadControl::Y];

/// One icon cell: 64 pixels of 20 twips, the size of the title's own icon
/// pictures (Series 1.0.2.1).
const CELL_TWIPS: i32 = 1280;
/// The cap's distance from the cell's edge and its corner radius.
const CAP_MARGIN_TWIPS: i32 = 50;
const CAP_RADIUS_TWIPS: i32 = 220;
/// The label's distance from the cell's edge.
const LABEL_MARGIN_TWIPS: i32 = 250;
/// The tallest a row of a label is drawn: half the cell, so that a letter
/// and a sign of one key are drawn the same height.
const TALLEST_ROW_TWIPS: f64 = 640.0;
/// The distance between a label's two rows, in ems.
const ROW_EMS: f64 = 1.05;

const WHITE: [u8; 4] = [0xFF; 4];
const BLACK: [u8; 4] = [0, 0, 0, 0xFF];

/// A position in ems, x right; y up from the baseline in a glyph, down from
/// the first row's baseline once placed in a label.
pub(crate) type EmPoint = [f64; 2];

/// A character's outline, outer contours clockwise, and how far the pen
/// moves after it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Glyph {
    pub(crate) contours: Vec<Contour<EmPoint>>,
    pub(crate) advance_ems: f64,
}

/// What a key cap shows for a virtual key; `None` for a key without one.
fn key_label(virtual_key: u8) -> Option<&'static str> {
    const LETTERS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    Some(match virtual_key {
        VK_BACK => "\u{232B}",
        VK_TAB => "Tab",
        VK_RETURN => "\u{21B5}",
        VK_ESCAPE => "Esc",
        VK_LEFT => "\u{2190}",
        VK_UP => "\u{2191}",
        VK_RIGHT => "\u{2192}",
        VK_DOWN => "\u{2193}",
        VK_SPACE => "Space",
        VK_LSHIFT => "Shift",
        VK_LCONTROL => "Ctrl",
        VK_RSHIFT => "RShift",
        VK_RCONTROL => "RCtrl",
        VK_LMENU | VK_RMENU => "Alt",
        VK_CAPITAL => "Caps",
        VK_LBUTTON => "LMB",
        VK_RBUTTON => "RMB",
        VK_MBUTTON => "MMB",
        VK_XBUTTON1 => "M4",
        VK_XBUTTON2 => "M5",
        b'0'..=b'9' => {
            const DIGITS: &str = "0123456789";
            let at = usize::from(virtual_key - b'0');
            DIGITS.get(at..=at)?
        }
        0x70..=0x7B => {
            const F_KEYS: [&str; 12] = [
                "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
            ];
            F_KEYS.get(usize::from(virtual_key - 0x70))?
        }
        b'A'..=b'Z' => {
            let at = usize::from(virtual_key - b'A');
            LETTERS.get(at..=at)?
        }
        _ => return None,
    })
}

/// The key cap: a rounded square, clockwise.
fn cap() -> Contour {
    let (low, high) = (CAP_MARGIN_TWIPS, CELL_TWIPS - CAP_MARGIN_TWIPS);
    let (near, far) = (low + CAP_RADIUS_TWIPS, high - CAP_RADIUS_TWIPS);
    let point = |x, y| Point { x, y };
    let corner = |cx, cy, x, y| Edge::Curve {
        control: point(cx, cy),
        to: point(x, y),
    };
    Contour {
        start: point(near, low),
        edges: vec![
            Edge::Line(point(far, low)),
            corner(high, low, high, near),
            Edge::Line(point(high, far)),
            corner(high, high, far, high),
            Edge::Line(point(near, high)),
            corner(low, high, low, far),
            Edge::Line(point(low, near)),
            corner(low, low, near, low),
        ],
    }
}

/// The labels of `keys` placed in the cell, from `glyphs`: in one row, or
/// in two rows of two keys for four. `None` when a character has no glyph.
fn label(keys: &[&str], glyphs: &HashMap<char, Glyph>) -> Option<Vec<Contour>> {
    let rows: Vec<String> = if keys.len() == 4 {
        keys.chunks(2).map(<[&str]>::concat).collect()
    } else {
        vec![keys.concat()]
    };
    // Each row's contours, x from the row's start, y down from the first
    // row's baseline.
    let mut placed: Vec<Vec<Contour<EmPoint>>> = Vec::new();
    let mut down = 0.0;
    for row in rows {
        let mut contours = Vec::new();
        let mut pen = 0.0;
        for character in row.chars() {
            let glyph = glyphs.get(&character)?;
            let place = |[x, y]: EmPoint| [x + pen, down - y];
            contours.extend(glyph.contours.iter().map(|contour| contour.map(place)));
            pen += glyph.advance_ems;
        }
        placed.push(contours);
        down += ROW_EMS;
    }
    let extent = |contours: &[Contour<EmPoint>], axis: usize| {
        contours
            .iter()
            .flat_map(Contour::points)
            .fold((f64::MAX, f64::MIN), |(low, high), point| {
                (low.min(point[axis]), high.max(point[axis]))
            })
    };
    let widths: Vec<(f64, f64)> = placed.iter().map(|row| extent(row, 0)).collect();
    let heights: Vec<(f64, f64)> = placed.iter().map(|row| extent(row, 1)).collect();
    let widest = widths
        .iter()
        .map(|(low, high)| high - low)
        .fold(0.0, f64::max);
    let tallest_row = heights
        .iter()
        .map(|(low, high)| high - low)
        .fold(0.0, f64::max);
    let (top, bottom) = heights
        .iter()
        .fold((f64::MAX, f64::MIN), |(low, high), &(a, b)| {
            (low.min(a), high.max(b))
        });
    if !(widest > 0.0 && tallest_row > 0.0) {
        return None;
    }
    let room = f64::from(CELL_TWIPS - 2 * LABEL_MARGIN_TWIPS);
    let scale = (room / widest)
        .min(room / (bottom - top))
        .min(TALLEST_ROW_TWIPS / tallest_row);
    let middle = f64::from(CELL_TWIPS) / 2.0;
    let mut contours = Vec::new();
    for (row, (low, high)) in placed.iter().zip(&widths) {
        // Inside the cell by construction, so the conversion keeps the value.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a position inside one icon cell"
        )]
        let twips = |[x, y]: EmPoint| Point {
            x: (middle + (x - (low + high) / 2.0) * scale).round() as i32,
            y: (middle + (y - (top + bottom) / 2.0) * scale).round() as i32,
        };
        contours.extend(row.iter().map(|contour| contour.map(twips)));
    }
    Some(contours)
}

/// The icon number a class name ends in, after its last `_`.
fn icon_number(name: &[u8]) -> Option<u32> {
    let digits = name.get(name.iter().rposition(|&byte| byte == b'_')? + 1..)?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// The shapes a sprite places, back to front, when it is one still frame of
/// placements; `None` when it changes over time or masks.
fn still_shapes(sprite: &Sprite) -> Option<Vec<u16>> {
    let mut placed: Vec<(Depth, u16)> = sprite
        .placements
        .iter()
        .map(|placement| match placement.change {
            Change::Place(character) if placement.frame == 0 => Some((placement.depth, character)),
            Change::Place(_)
            | Change::Mask(_)
            | Change::Replace(_)
            | Change::Modify
            | Change::Remove
            | Change::Unread => None,
        })
        .collect::<Option<_>>()?;
    placed.sort_unstable();
    if placed.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return None;
    }
    Some(placed.into_iter().map(|(_, shape)| shape).collect())
}

/// The controls an icon number pictures.
fn controls(number: u32) -> Option<&'static [PadControl]> {
    ICONS
        .iter()
        .find(|(known, _)| *known == number)
        .map(|(_, controls)| *controls)
}

/// How many times each character is placed, anywhere in a movie. `None`
/// when a placement does not say what it places.
fn uses(sprites: &[Sprite], timeline: &[Placement]) -> Option<HashMap<u16, u32>> {
    let mut uses = HashMap::new();
    let placements = sprites
        .iter()
        .flat_map(|sprite| &sprite.placements)
        .chain(timeline);
    for placement in placements {
        match placement.change {
            Change::Place(character) | Change::Mask(character) | Change::Replace(character) => {
                *uses.entry(character).or_insert(0) += 1;
            }
            Change::Modify | Change::Remove => {}
            Change::Unread => return None,
        }
    }
    Some(uses)
}

/// Why a movie was not redrawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// It is not the icon movie.
    NotIconMovie,
    /// It is the icon movie, but it places something this module cannot read.
    Unreadable,
    /// It is the icon movie, but no icon of it could be drawn: a glyph is
    /// missing, or a shape is drawn elsewhere too.
    NothingDrawn,
}

/// The key the keyboard layer's layout in force binds to `control`, at the
/// time the movie is drawn (see [`pad::set_key_source`]).
#[cfg_attr(
    not(any(windows, test)),
    expect(dead_code, reason = "only the Windows hook draws key caps")
)]
pub(crate) fn active_key(control: PadControl) -> Option<u8> {
    pad::bound_key(control)
}

/// What a key cap shows for `control` now; `None` when nothing is bound to
/// it or the key has no label.
#[must_use]
pub fn control_label(control: PadControl) -> Option<&'static str> {
    key_label(active_key(control)?)
}

/// [`key_caps_with`] with the classic keys.
#[cfg(test)]
pub(crate) fn key_caps(
    movie: &[u8],
    glyph: &dyn Fn(char) -> Option<Glyph>,
) -> Result<Vec<u8>, Refusal> {
    key_caps_with(movie, glyph, &|control| Some(pad::classic_key(control)))
}

/// `movie` with each known icon drawn as a key cap, labelled with `glyph`'s
/// outlines and the key `key` binds to the icon's control. An icon whose
/// control has no key keeps its picture.
#[cfg_attr(
    not(any(windows, test)),
    expect(dead_code, reason = "only the Windows hook draws key caps")
)]
pub(crate) fn key_caps_with(
    movie: &[u8],
    glyph: &dyn Fn(char) -> Option<Glyph>,
    key: &dyn Fn(PadControl) -> Option<u8>,
) -> Result<Vec<u8>, Refusal> {
    let movie = Movie::parse(movie).ok_or(Refusal::NotIconMovie)?;
    // The class table first: it alone tells most movies apart. The first
    // class of each known number names that icon's sprite.
    let mut numbers = HashSet::new();
    let mut wanted: HashMap<u16, &'static [PadControl]> = HashMap::new();
    for (sprite, name) in movie.classes().ok_or(Refusal::NotIconMovie)? {
        let Some((number, controls)) = icon_number(name).and_then(|n| Some((n, controls(n)?)))
        else {
            continue;
        };
        if numbers.insert(number) {
            wanted.entry(sprite).or_insert(controls);
        }
    }
    let has_icon = |button: PadControl| {
        ICONS
            .iter()
            .any(|&(number, controls)| controls == [button] && numbers.contains(&number))
    };
    if numbers.len() < FEWEST_ICONS || !FACE_BUTTONS.into_iter().all(has_icon) {
        return Err(Refusal::NotIconMovie);
    }
    if movie.defines_buttons() {
        return Err(Refusal::Unreadable);
    }
    let sprites = movie.sprites().ok_or(Refusal::Unreadable)?;
    let timeline = movie.timeline().ok_or(Refusal::Unreadable)?;
    let uses = uses(&sprites, &timeline).ok_or(Refusal::Unreadable)?;
    let icons: Vec<(&[PadControl], Vec<u16>)> = sprites
        .iter()
        .filter_map(|sprite| Some((wanted.remove(&sprite.id)?, still_shapes(sprite)?)))
        .collect();
    if icons.iter().filter(|(_, shapes)| shapes.len() >= 2).count() < FEWEST_ICONS {
        return Err(Refusal::NotIconMovie);
    }
    let definitions = movie.shape_counts();
    let keys_of = |controls: &[PadControl]| {
        controls
            .iter()
            .map(|&control| key_label(key(control)?))
            .collect::<Option<Vec<_>>>()
    };
    // The font is asked for each character once.
    let mut asked = HashSet::new();
    let mut glyphs: HashMap<char, Glyph> = HashMap::new();
    for character in icons
        .iter()
        .filter_map(|(controls, _)| keys_of(controls))
        .flatten()
        .flat_map(str::chars)
    {
        if asked.insert(character)
            && let Some(found) = glyph(character)
        {
            glyphs.insert(character, found);
        }
    }
    let mut bodies: Vec<ShapeBody> = Vec::new();
    for (controls, shapes) in &icons {
        // The back shape becomes the cap, the next one the label, and any
        // further ones nothing. An icon keeps its picture when a shape of it
        // is drawn elsewhere too, or is not one shape this module writes.
        let [back, front, rest @ ..] = shapes.as_slice() else {
            continue;
        };
        if shapes
            .iter()
            .any(|shape| uses.get(shape) != Some(&1) || definitions.get(shape) != Some(&1))
        {
            continue;
        }
        let Some(keys) = keys_of(controls) else {
            continue;
        };
        let Some(label) = label(&keys, &glyphs).and_then(|label| gfx::shape(*front, BLACK, &label))
        else {
            continue;
        };
        // Fill style 0, as the title's own shapes have it: with the cap's
        // colour as style 1, the first screen stayed black in one run.
        bodies.push(gfx::shape(*back, WHITE, &[cap()]).ok_or(Refusal::Unreadable)?);
        bodies.push(label);
        for &shape in rest {
            bodies.push(gfx::shape(shape, BLACK, &[]).ok_or(Refusal::Unreadable)?);
        }
    }
    if bodies.is_empty() {
        return Err(Refusal::NothingDrawn);
    }
    movie.replace_shapes(&bodies).ok_or(Refusal::Unreadable)
}

/// Hand-built movies for the tests of this module and of [`hook`].
#[cfg(test)]
pub(crate) mod fixtures {
    use super::ICONS;

    pub(crate) const END: u16 = 0;
    pub(crate) const SHOW_FRAME: u16 = 1;
    pub(crate) const PLACE_OBJECT_2: u16 = 26;
    pub(crate) const DEFINE_SHAPE_3: u16 = 32;
    pub(crate) const DEFINE_SPRITE: u16 = 39;
    pub(crate) const PLACE_OBJECT_3: u16 = 70;
    pub(crate) const SYMBOL_CLASS: u16 = 76;

    /// The first sprite id the fixtures give.
    pub(crate) const FIRST_SPRITE: u16 = 60_000;

    pub(crate) fn tag(code: u16, body: &[u8]) -> Vec<u8> {
        let mut bytes = ((code << 6) | 0x3F).to_le_bytes().to_vec();
        bytes.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        bytes.extend(body);
        bytes
    }

    /// A placement of `shape` at `depth`, with a matrix.
    pub(crate) fn place(depth: u16, shape: u16) -> Vec<u8> {
        let mut body = vec![0b110];
        body.extend(depth.to_le_bytes());
        body.extend(shape.to_le_bytes());
        tag(PLACE_OBJECT_2, &body)
    }

    /// A placement of `shape` at `depth` as a mask over deeper ones.
    pub(crate) fn mask(depth: u16, shape: u16) -> Vec<u8> {
        let mut body = vec![0x46];
        body.extend(depth.to_le_bytes());
        body.extend(shape.to_le_bytes());
        tag(PLACE_OBJECT_2, &body)
    }

    /// The body a hand-built movie gives the shape `id`.
    pub(crate) fn picture(id: u16) -> Vec<u8> {
        [id.to_le_bytes(), [0xAB, 0xCD]].concat()
    }

    /// A movie of `shapes`, then sprites of their own tags, then a class
    /// table naming characters.
    pub(crate) fn movie(
        shapes: &[u16],
        sprites: &[(u16, Vec<Vec<u8>>)],
        classes: &[(u16, String)],
    ) -> Vec<u8> {
        let mut bytes = b"GFX\x0b\0\0\0\0\0\0\x1e\x01\0".to_vec();
        for &shape in shapes {
            bytes.extend(tag(DEFINE_SHAPE_3, &picture(shape)));
        }
        for (id, own) in sprites {
            let mut body = [id.to_le_bytes(), [0, 0]].concat();
            body.extend(own.concat());
            body.extend(tag(END, &[]));
            bytes.extend(tag(DEFINE_SPRITE, &body));
        }
        let mut table = u16::try_from(classes.len()).unwrap().to_le_bytes().to_vec();
        for (id, name) in classes {
            table.extend(id.to_le_bytes());
            table.extend(name.bytes());
            table.push(0);
        }
        bytes.extend(tag(SYMBOL_CLASS, &table));
        bytes.extend(tag(END, &[]));
        let len = u32::try_from(bytes.len()).unwrap().to_le_bytes();
        bytes[4..8].copy_from_slice(&len);
        bytes
    }

    /// Every known icon with three shapes of its own, placed back to
    /// front: 100 times its number, and the two after it.
    pub(crate) fn known_icons() -> Vec<(u32, Vec<u16>)> {
        ICONS
            .iter()
            .map(|&(number, _)| {
                let first = 100 * u16::try_from(number).unwrap();
                (number, vec![first, first + 1, first + 2])
            })
            .collect()
    }

    /// The parts of a movie, for a test to change before it is built.
    pub(crate) struct Parts {
        pub(crate) shapes: Vec<u16>,
        pub(crate) sprites: Vec<(u16, Vec<Vec<u8>>)>,
        pub(crate) classes: Vec<(u16, String)>,
    }

    impl Parts {
        pub(crate) fn movie(&self) -> Vec<u8> {
            movie(&self.shapes, &self.sprites, &self.classes)
        }
    }

    /// The parts of a movie of `icons`, each a sprite placing its shapes at
    /// depths 1, 2 and so on, with a class named for its number.
    pub(crate) fn parts(icons: &[(u32, Vec<u16>)]) -> Parts {
        let mut shapes: Vec<u16> = icons
            .iter()
            .flat_map(|(_, shapes)| shapes.clone())
            .collect();
        shapes.sort_unstable();
        shapes.dedup();
        let sprite = |index: usize| FIRST_SPRITE + u16::try_from(index).unwrap();
        let sprites = icons
            .iter()
            .enumerate()
            .map(|(index, (_, shapes))| {
                let placed = (1..).zip(shapes).map(|(depth, &shape)| place(depth, shape));
                (sprite(index), placed.collect())
            })
            .collect();
        let classes = icons
            .iter()
            .enumerate()
            .map(|(index, (number, _))| (sprite(index), format!("icon_{number}")))
            .collect();
        Parts {
            shapes,
            sprites,
            classes,
        }
    }

    pub(crate) fn icon_movie(icons: &[(u32, Vec<u16>)]) -> Vec<u8> {
        parts(icons).movie()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use super::pad::{PadControl, VK_BACK, VK_RETURN, classic_key};
    use victory_formats::gfx::{Contour, Edge, Point};

    use super::fixtures::{
        DEFINE_SHAPE_3, PLACE_OBJECT_3, SHOW_FRAME, icon_movie, known_icons, mask, parts, picture,
        place, tag,
    };
    use super::{Glyph, ICONS, Refusal, controls, icon_number, key_caps, key_label, label};

    /// A square half an em wide on the baseline, clockwise with y up.
    fn square(_: char) -> Option<Glyph> {
        Some(Glyph {
            contours: vec![Contour {
                start: [0.0, 0.0],
                edges: [[0.0, 0.5], [0.5, 0.5], [0.5, 0.0], [0.0, 0.0]]
                    .map(Edge::Line)
                    .to_vec(),
            }],
            advance_ems: 0.6,
        })
    }

    /// A square glyph for each character of `text`.
    fn squares(text: &str) -> HashMap<char, Glyph> {
        text.chars()
            .filter_map(|character| Some((character, square(character)?)))
            .collect()
    }

    /// `movie` with `tag` as its first tag, after its 13-byte header.
    fn with_first_tag(movie: &[u8], tag: &[u8]) -> Vec<u8> {
        let mut bytes = [&movie[..13], tag, &movie[13..]].concat();
        let len = u32::try_from(bytes.len()).unwrap().to_le_bytes();
        bytes[4..8].copy_from_slice(&len);
        bytes
    }

    /// The tags of a fixture movie (all with long headers) after its
    /// 13-byte header: code and body.
    fn fixture_tags(movie: &[u8]) -> Vec<(u16, &[u8])> {
        let mut found = Vec::new();
        let mut at = 13;
        while at < movie.len() {
            let code = u16::from_le_bytes([movie[at], movie[at + 1]]) >> 6;
            let len = u32::from_le_bytes(movie[at + 2..at + 6].try_into().unwrap());
            let end = at + 6 + usize::try_from(len).unwrap();
            found.push((code, &movie[at + 6..end]));
            at = end;
        }
        found
    }

    /// The body of each shape tag of a fixture movie, by the shape's id.
    fn shape_bodies(movie: &[u8]) -> Vec<(u16, &[u8])> {
        fixture_tags(movie)
            .into_iter()
            .filter(|&(code, _)| code == DEFINE_SHAPE_3)
            .map(|(_, body)| (u16::from_le_bytes([body[0], body[1]]), body))
            .collect()
    }

    /// The fill a written shape has, after its id and 7 bytes of bounds.
    fn fill(body: &[u8]) -> &[u8] {
        &body[9..17]
    }

    const WHITE_FILL: [u8; 8] = [1, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0x10];
    const BLACK_FILL: [u8; 8] = [1, 0, 0, 0, 0, 0xFF, 0, 0x10];

    /// Checks that the shapes `redrawn` names were drawn anew and every
    /// other shape kept the fixtures' picture.
    fn redrawn_only(movie: &[u8], redrawn: impl Fn(u16) -> bool) {
        for (id, body) in shape_bodies(movie) {
            assert_eq!(body != picture(id), redrawn(id), "{id}");
        }
    }

    #[test]
    fn every_known_icon_becomes_a_white_cap_under_a_black_label() {
        let mut icons = known_icons();
        // An icon whose number is not a gamepad control keeps its picture.
        icons.push((3, vec![1, 2]));
        let original = icon_movie(&icons);

        let redrawn = key_caps(&original, &square).unwrap();

        let bodies = shape_bodies(&redrawn);
        assert_eq!(bodies.len(), 3 * ICONS.len() + 2);
        for (id, body) in bodies {
            let empty = [
                &id.to_le_bytes()[..],
                &[0x08, 0x00, 1, 0, 0, 0, 0, 0xFF, 0, 0x10, 0x00],
            ]
            .concat();
            match (id < 100, id % 100) {
                (true, _) => assert_eq!(body, picture(id), "{id}"),
                (false, 0) => assert_eq!(fill(body), WHITE_FILL, "{id}"),
                (false, 1) => assert_eq!(fill(body), BLACK_FILL, "{id}"),
                (false, _) => assert_eq!(body, empty, "{id}"),
            }
        }
        assert_eq!(
            redrawn[4..8],
            u32::try_from(redrawn.len()).unwrap().to_le_bytes()
        );
        // Sprites, classes and the end tag are as they were.
        let others = |movie| {
            fixture_tags(movie)
                .into_iter()
                .filter(|&(code, _)| code != DEFINE_SHAPE_3)
                .map(|(code, body)| (code, body.to_vec()))
                .collect::<Vec<_>>()
        };
        assert_eq!(others(&redrawn), others(&original));
    }

    #[test]
    fn a_movie_with_few_known_icons_is_not_the_icon_movie() {
        // 19 known numbers, the face buttons among them.
        let mut icons = known_icons();
        icons.retain(|&(number, _)| number >= 21);
        icons.truncate(19);
        icons.push((3, vec![1, 2]));

        assert_eq!(
            key_caps(&icon_movie(&icons), &square),
            Err(Refusal::NotIconMovie)
        );
        assert_eq!(key_caps(b"GFX", &square), Err(Refusal::NotIconMovie));
    }

    #[test]
    fn a_movie_with_few_icons_of_two_shapes_is_not_the_icon_movie() {
        // Every number is known, but each icon is a single shape.
        let icons: Vec<(u32, Vec<u16>)> = known_icons()
            .into_iter()
            .map(|(number, shapes)| (number, shapes[..1].to_vec()))
            .collect();

        assert_eq!(
            key_caps(&icon_movie(&icons), &square),
            Err(Refusal::NotIconMovie)
        );
    }

    #[test]
    fn a_movie_with_a_button_leaves_its_icons_alone() {
        // A button's records name characters too.
        let button = tag(34, &[1, 0, 0, 0, 0, 0]);
        let movie = with_first_tag(&icon_movie(&known_icons()), &button);

        assert_eq!(key_caps(&movie, &square), Err(Refusal::Unreadable));
    }

    #[test]
    fn a_movie_without_the_face_buttons_is_not_the_icon_movie() {
        let mut icons = known_icons();
        icons.retain(|&(number, _)| number != 33);

        assert_eq!(
            key_caps(&icon_movie(&icons), &square),
            Err(Refusal::NotIconMovie)
        );
    }

    #[test]
    fn a_movie_where_no_icon_can_be_drawn_is_left_alone() {
        assert_eq!(
            key_caps(&icon_movie(&known_icons()), &|_| None),
            Err(Refusal::NothingDrawn)
        );
    }

    #[test]
    fn many_classes_and_sprites_are_looked_up_once_each() {
        // Classes that each named every sprite would take minutes here.
        let mut parts = parts(&known_icons());
        for id in 1..=59_999u16 {
            parts.classes.push((id, "extra_4".to_owned()));
            parts.sprites.push((id, Vec::new()));
        }

        let movie = parts.movie();

        let started = Instant::now();
        let redrawn = key_caps(&movie, &square);

        assert!(redrawn.is_ok());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn an_icon_that_shares_a_shape_with_any_sprite_keeps_its_picture() {
        let mut parts = parts(&known_icons());
        // A sprite with no class draws icon 4's back shape too.
        parts.sprites.push((100, vec![place(1, 400)]));

        let redrawn = key_caps(&parts.movie(), &square).unwrap();

        redrawn_only(&redrawn, |id| id / 100 != 4);
    }

    #[test]
    fn an_icon_placing_something_other_than_a_shape_keeps_its_picture() {
        let mut parts = parts(&known_icons());
        // Icon 4's label is no shape of the movie.
        parts.shapes.retain(|&shape| shape != 401);

        let redrawn = key_caps(&parts.movie(), &square).unwrap();

        redrawn_only(&redrawn, |id| id / 100 != 4);
    }

    #[test]
    fn an_icon_whose_shape_is_defined_twice_keeps_its_picture() {
        let mut parts = parts(&known_icons());
        parts.shapes.push(400);

        let redrawn = key_caps(&parts.movie(), &square).unwrap();

        redrawn_only(&redrawn, |id| id / 100 != 4);
    }

    #[test]
    fn an_icon_with_a_mask_keeps_its_picture() {
        let mut parts = parts(&known_icons());
        parts.sprites[0].1[2] = mask(3, 402);

        let redrawn = key_caps(&parts.movie(), &square).unwrap();

        redrawn_only(&redrawn, |id| id / 100 != 4);
    }

    #[test]
    fn the_back_shape_is_found_by_depth_not_by_tag_order_or_id() {
        let icons = known_icons();
        let mut parts = parts(&icons);
        // Every icon places its shapes in id order, at depths 3, 2 and 1.
        for ((_, own), (_, shapes)) in parts.sprites.iter_mut().zip(&icons) {
            *own = (1..=3)
                .rev()
                .zip(shapes)
                .map(|(depth, &shape)| place(depth, shape))
                .collect();
        }

        let redrawn = key_caps(&parts.movie(), &square).unwrap();

        for (id, body) in shape_bodies(&redrawn) {
            match id % 100 {
                2 => assert_eq!(fill(body), WHITE_FILL, "{id}"),
                1 => assert_eq!(fill(body), BLACK_FILL, "{id}"),
                _ => assert_eq!(body.len(), 13, "{id}"),
            }
        }
    }

    #[test]
    fn an_icon_that_changes_over_frames_keeps_its_picture() {
        let mut parts = parts(&known_icons());
        // Icon 4 places its last shape in a second frame.
        parts.sprites[0].1.insert(2, tag(SHOW_FRAME, &[]));

        let redrawn = key_caps(&parts.movie(), &square).unwrap();

        redrawn_only(&redrawn, |id| id / 100 != 4);
    }

    #[test]
    fn a_placement_that_cannot_be_read_leaves_the_movie_alone() {
        let mut parts = parts(&known_icons());
        parts
            .sprites
            .push((100, vec![tag(PLACE_OBJECT_3, &[0, 0, 1, 0])]));

        assert_eq!(key_caps(&parts.movie(), &square), Err(Refusal::Unreadable));
    }

    #[test]
    fn a_second_icon_with_a_number_keeps_its_picture() {
        let mut icons = known_icons();
        icons.push((30, vec![1, 2]));

        let redrawn = key_caps(&icon_movie(&icons), &square).unwrap();

        redrawn_only(&redrawn, |id| id >= 100);
    }

    #[test]
    fn an_icon_whose_key_has_no_glyph_keeps_its_picture() {
        let no_arrows = |character: char| {
            if ('\u{2190}'..='\u{2193}').contains(&character) {
                None
            } else {
                square(character)
            }
        };

        let redrawn = key_caps(&icon_movie(&known_icons()), &no_arrows).unwrap();

        // Icons 4 to 11 are the direction pad's, on the arrow keys.
        redrawn_only(&redrawn, |id| !(4..=11).contains(&(id / 100)));
    }

    #[test]
    fn the_enter_key_labels_the_a_button_and_backspace_the_b_button() {
        let only = |wanted: char| {
            move |character: char| {
                if character == wanted {
                    square(character)
                } else {
                    None
                }
            }
        };

        let enter = key_caps(&icon_movie(&known_icons()), &only('\u{21B5}')).unwrap();
        let backspace = key_caps(&icon_movie(&known_icons()), &only('\u{232B}')).unwrap();

        redrawn_only(&enter, |id| id / 100 == 30);
        redrawn_only(&backspace, |id| id / 100 == 31);
        assert_eq!(key_label(VK_RETURN), Some("\u{21B5}"));
        assert_eq!(key_label(VK_BACK), Some("\u{232B}"));
        assert_eq!(controls(30), Some(&[PadControl::A][..]));
        assert_eq!(controls(31), Some(&[PadControl::B][..]));
    }

    #[test]
    fn each_character_is_asked_for_once() {
        let asked = Cell::new(0);
        let counting = |character| {
            asked.set(asked.get() + 1);
            square(character)
        };

        key_caps(&icon_movie(&known_icons()), &counting).unwrap();

        // Arrows 4, letters 16, "Esc" and "Tab" 5 more, Enter and Backspace 2.
        assert_eq!(asked.get(), 27);
    }

    #[test]
    fn every_key_of_the_stand_in_pad_has_a_label() {
        for control in ICONS.iter().flat_map(|(_, controls)| controls.iter()) {
            assert!(key_label(classic_key(*control)).is_some(), "{control:?}");
        }
        assert_eq!(key_label(0x1B), Some("Esc"));
        assert_eq!(key_label(b'Q'), Some("Q"));
        assert_eq!(key_label(0x70), Some("F1"));
        assert_eq!(key_label(0x88), None);
    }

    #[test]
    fn prompts_name_the_keys_the_layout_in_force_binds() {
        use super::pad::{VK_LCONTROL, VK_LSHIFT, VK_SPACE};
        use super::{PadControl as P, active_key, control_label};
        // A key source in the shape of the keyboard layer's: output names in,
        // virtual keys out.
        fn source(name: &'static str) -> Option<u8> {
            match name {
                "A" => Some(VK_SPACE),
                "LS" => Some(VK_LSHIFT),
                "RT" => Some(0x01),
                "DpadUp" => Some(VK_LCONTROL),
                "LeftUp" => Some(b'W'),
                _ => None,
            }
        }
        assert_eq!(active_key(P::A), None, "no source set yet");
        assert!(super::pad::set_key_source(source));
        assert_eq!(control_label(P::A), Some("Space"));
        assert_eq!(control_label(P::LeftStick), Some("Shift"));
        assert_eq!(control_label(P::RightTrigger), Some("LMB"));
        assert_eq!(control_label(P::Up), Some("Ctrl"));
        assert_eq!(control_label(P::LeftStickUp), Some("W"));
        assert_eq!(control_label(P::X), None, "nothing bound");
    }

    #[test]
    fn every_control_name_is_a_keyboard_layer_output() {
        // The names the keyboard layer's `OUTPUTS` table has for pad controls.
        const OUTPUTS: [&str; 24] = [
            "A", "B", "X", "Y", "LB", "RB", "LT", "RT", "LS", "RS", "View", "Menu", "DpadUp",
            "DpadDown", "DpadLeft", "DpadRight", "LeftUp", "LeftDown", "LeftLeft", "LeftRight",
            "RightUp", "RightDown", "RightLeft", "RightRight",
        ];
        for control in PadControl::ALL {
            assert!(OUTPUTS.contains(&control.output_name()), "{control:?}");
        }
    }

    #[test]
    fn an_icon_without_a_key_keeps_its_picture() {
        use super::key_caps_with;
        // Nothing binds X: its icon is left alone, the others redrawn.
        let key = |c| (c != PadControl::X).then(|| classic_key(c));
        let redrawn = key_caps_with(&icon_movie(&known_icons()), &square, &key).unwrap();
        let has_keys = |id: u16| {
            controls(u32::from(id / 100)).is_some_and(|c| c.iter().all(|&c| key(c).is_some()))
        };
        assert!(!has_keys(3200), "X has no key here");
        redrawn_only(&redrawn, has_keys);
    }

    fn line(x: i32, y: i32) -> Edge {
        Edge::Line(Point { x, y })
    }

    #[test]
    fn one_character_is_centred_at_the_tallest_size() {
        // Half an em drawn 640 twips tall, around the middle of a 1280-twip
        // cell.
        assert_eq!(
            label(&["A"], &squares("A")),
            Some(vec![Contour {
                start: Point { x: 320, y: 960 },
                edges: vec![
                    line(320, 320),
                    line(960, 320),
                    line(960, 960),
                    line(320, 960)
                ],
            }])
        );
    }

    #[test]
    fn four_keys_are_set_in_two_rows_inside_the_margin() {
        let contours = label(&["A", "B", "C", "D"], &squares("ABCD")).unwrap();

        // 1.55 ems of height fill the 780 twips between the margins.
        let starts: Vec<Point> = contours.iter().map(|contour| contour.start).collect();
        assert_eq!(
            starts,
            [
                Point { x: 363, y: 502 },
                Point { x: 665, y: 502 },
                Point { x: 363, y: 1030 },
                Point { x: 665, y: 1030 },
            ]
        );
        assert_eq!(
            contours[0].edges,
            [
                line(363, 250),
                line(615, 250),
                line(615, 502),
                line(363, 502)
            ]
        );
    }

    #[test]
    fn two_keys_of_four_characters_stay_in_one_row() {
        let contours = label(&["Esc", "\u{21B5}"], &squares("Esc\u{21B5}")).unwrap();

        assert_eq!(contours.len(), 4);
        assert!(
            contours
                .iter()
                .all(|contour| contour.start.y == contours[0].start.y)
        );
    }

    #[test]
    fn a_label_with_a_character_without_a_glyph_is_not_drawn() {
        assert_eq!(label(&["A", "B"], &squares("A")), None);
        assert_eq!(label(&[], &squares("A")), None);
    }

    #[test]
    fn a_class_name_gives_the_number_after_its_last_underscore() {
        assert_eq!(icon_number(b"icon_12"), Some(12));
        assert_eq!(icon_number(b"a_b_7"), Some(7));
        assert_eq!(icon_number(b"7"), None);
        assert_eq!(icon_number(b"icon_"), None);
        assert_eq!(icon_number(b"icon_1a"), None);
        assert_eq!(icon_number(b""), None);
        assert_eq!(icon_number(b"icon_99999999999"), None);
    }
}
