//! Scaleform GFx movies: the `GFX` container around a SWF tag stream.
//!
//! Reads an uncompressed movie's shapes, sprites, placements and class
//! names, writes solid vector shapes, and builds a copy of a movie with some
//! of its shapes replaced. It does not read compressed (`CFX`) movies, and it
//! does not interpret scripts, fonts, bitmaps or placement matrices.

use std::collections::HashMap;
use std::ops::Range;

/// Tag codes of the SWF tag stream.
const END: u16 = 0;
const SHOW_FRAME: u16 = 1;
const PLACE_OBJECT: u16 = 4;
const REMOVE_OBJECT: u16 = 5;
const DEFINE_BUTTON: u16 = 7;
const DEFINE_BUTTON_2: u16 = 34;
const PLACE_OBJECT_2: u16 = 26;
const REMOVE_OBJECT_2: u16 = 28;
const DEFINE_SHAPE_3: u16 = 32;
const DEFINE_SPRITE: u16 = 39;
const PLACE_OBJECT_3: u16 = 70;
const SYMBOL_CLASS: u16 = 76;

/// A tag header's length field when the real length follows as a `u32`.
const LONG_TAG: u16 = 0x3F;
/// `PlaceObject2` flags: the placement changes the character at its depth,
/// and it names a character.
const PLACE_MOVES: u8 = 1;
const PLACE_NAMES_CHARACTER: u8 = 2;
/// A `PlaceObject2` flag: the placed character is a mask over deeper ones.
const PLACE_CLIPS: u8 = 0x40;
/// Style change flags of a shape record.
const MOVE_TO: u32 = 0b00001;
const FILL_STYLE_0: u32 = 0b00010;
/// The bytes of a movie's header: `GFX`, a version and the movie's length.
pub const HEADER_BYTES: usize = 8;
/// The widest signed field a shape's bounds or a move can have, bits.
const WIDEST_FIELD_BITS: u32 = 31;
/// The widest signed delta an edge can have, bits (a 4-bit count plus 2).
const WIDEST_EDGE_BITS: u32 = 17;

/// A position in twips (a twentieth of a pixel), y down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// One edge of a contour, from where the previous one ended, in points of
/// some kind `P` (twips for a shape).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge<P = Point> {
    Line(P),
    /// A quadratic curve.
    Curve {
        control: P,
        to: P,
    },
}

impl<P: Copy> Edge<P> {
    /// The edge's control and end; a line's control is its end.
    pub fn points(self) -> [P; 2] {
        match self {
            Self::Line(to) => [to, to],
            Self::Curve { control, to } => [control, to],
        }
    }

    /// The same edge with every point moved by `place`.
    pub fn map<Q>(self, place: impl Fn(P) -> Q) -> Edge<Q> {
        match self {
            Self::Line(to) => Edge::Line(place(to)),
            Self::Curve { control, to } => Edge::Curve {
                control: place(control),
                to: place(to),
            },
        }
    }
}

/// One closed outline, in points of some kind `P` (twips for a shape).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contour<P = Point> {
    pub start: P,
    pub edges: Vec<Edge<P>>,
}

impl<P: Copy> Contour<P> {
    /// Its start, then each edge's control and end.
    pub fn points(&self) -> impl Iterator<Item = P> + '_ {
        std::iter::once(self.start).chain(self.edges.iter().flat_map(|edge| edge.points()))
    }

    /// The same outline with every point moved by `place`.
    pub fn map<Q>(&self, place: impl Fn(P) -> Q) -> Contour<Q> {
        Contour {
            start: place(self.start),
            edges: self.edges.iter().map(|edge| edge.map(&place)).collect(),
        }
    }

    /// Whether its last edge ends where it starts.
    fn closed(&self) -> bool
    where
        P: PartialEq,
    {
        self.edges
            .last()
            .is_some_and(|edge| edge.points()[1] == self.start)
    }
}

/// The body of a `DefineShape3` tag, made by [`shape`]. It carries the id
/// of the shape it defines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeBody {
    id: u16,
    bytes: Vec<u8>,
}

/// What one placement tag does at its depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// A character placed at a free depth.
    Place(u16),
    /// A character placed at a free depth as a mask over deeper ones.
    Mask(u16),
    /// The character at the depth replaced by this one.
    Replace(u16),
    /// The character at the depth moved or recoloured.
    Modify,
    /// The character at the depth removed.
    Remove,
    /// A placement this module does not read (`PlaceObject3`).
    Unread,
}

/// A depth of a timeline: deeper characters are drawn first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Depth(pub u16);

/// One placement tag of a timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The frame it is in, counted from 0.
    pub frame: u16,
    pub depth: Depth,
    pub change: Change,
}

/// A sprite and its own timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    pub id: u16,
    pub placements: Vec<Placement>,
}

/// One tag of a tag stream: its code, where its header starts and its body.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Tag {
    code: u16,
    start: usize,
    body: Range<usize>,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

/// The tags in `range` of `bytes`, up to and including the first end tag.
/// `None` when a tag runs past the range.
fn tags(bytes: &[u8], range: Range<usize>) -> Option<Vec<Tag>> {
    let mut found = Vec::new();
    let mut at = range.start;
    while at < range.end {
        let head = u16_at(bytes, at)?;
        let code = head >> 6;
        let (body, len) = match head & LONG_TAG {
            LONG_TAG => (
                at.checked_add(6)?,
                usize::try_from(u32_at(bytes, at.checked_add(2)?)?).ok()?,
            ),
            short => (at.checked_add(2)?, usize::from(short)),
        };
        let end = body.checked_add(len).filter(|&end| end <= range.end)?;
        found.push(Tag {
            code,
            start: at,
            body: body..end,
        });
        if code == END {
            break;
        }
        at = end;
    }
    Some(found)
}

/// The placements among `tags` of `bytes`, frame by frame. `None` when a
/// placement's body is too short for its fields.
fn placements(bytes: &[u8], tags: &[Tag]) -> Option<Vec<Placement>> {
    let mut found = Vec::new();
    let mut frame = 0u16;
    for tag in tags {
        let body = bytes.get(tag.body.clone())?;
        let (depth, change) = match tag.code {
            SHOW_FRAME => {
                frame = frame.saturating_add(1);
                continue;
            }
            // A character, then a depth.
            PLACE_OBJECT => (u16_at(body, 2)?, Change::Place(u16_at(body, 0)?)),
            // Flags, a depth, then the character when the flags name one.
            PLACE_OBJECT_2 => {
                let flags = *body.first()?;
                let moves = flags & PLACE_MOVES != 0;
                let change = match (moves, flags & PLACE_NAMES_CHARACTER != 0) {
                    (false, true) if flags & PLACE_CLIPS != 0 => Change::Mask(u16_at(body, 3)?),
                    (false, true) => Change::Place(u16_at(body, 3)?),
                    (true, true) => Change::Replace(u16_at(body, 3)?),
                    (_, false) => Change::Modify,
                };
                (u16_at(body, 1)?, change)
            }
            // Two bytes of flags, then a depth.
            PLACE_OBJECT_3 => (u16_at(body, 2)?, Change::Unread),
            // A character, then a depth; or the depth alone.
            REMOVE_OBJECT => (u16_at(body, 2)?, Change::Remove),
            REMOVE_OBJECT_2 => (u16_at(body, 0)?, Change::Remove),
            _ => continue,
        };
        found.push(Placement {
            frame,
            depth: Depth(depth),
            change,
        });
    }
    Some(found)
}

/// Whether `header` starts an uncompressed movie `len` bytes long.
#[must_use]
pub fn names_movie(header: &[u8; HEADER_BYTES], len: usize) -> bool {
    let [b'G', b'F', b'X', _, length @ ..] = *header else {
        return false;
    };
    usize::try_from(u32::from_le_bytes(length)).is_ok_and(|length| length == len)
}

/// An uncompressed movie with its top-level tags listed.
pub struct Movie<'a> {
    bytes: &'a [u8],
    /// Where the first tag starts, after the header.
    first: usize,
    tags: Vec<Tag>,
    /// How many `DefineShape3` tags define each shape id.
    shape_counts: HashMap<u16, usize>,
}

impl<'a> Movie<'a> {
    /// Lists the tags of `bytes`. `None` when they are not an uncompressed
    /// movie, the length in the header is not theirs, or a tag runs past
    /// the end.
    #[must_use]
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        if !names_movie(bytes.first_chunk()?, bytes.len()) {
            return None;
        }
        // The frame rectangle: a 5-bit field width, then four fields; the
        // frame rate and the frame count follow it.
        let field_bits = usize::from(bytes.get(8)? >> 3);
        let first = 8 + (5 + 4 * field_bits).div_ceil(8) + 4;
        let tags = tags(bytes, first..bytes.len())?;
        let mut shape_counts = HashMap::new();
        let shapes = tags.iter().filter(|tag| tag.code == DEFINE_SHAPE_3);
        for id in shapes.filter_map(|tag| u16_at(bytes.get(tag.body.clone())?, 0)) {
            *shape_counts.entry(id).or_insert(0) += 1;
        }
        Some(Self {
            bytes,
            first,
            tags,
            shape_counts,
        })
    }

    fn bodies(&self, code: u16) -> impl Iterator<Item = &'a [u8]> {
        let bytes = self.bytes;
        self.tags
            .iter()
            .filter(move |tag| tag.code == code)
            .filter_map(move |tag| bytes.get(tag.body.clone()))
    }

    /// How many `DefineShape3` tags define each shape id.
    #[must_use]
    pub fn shape_counts(&self) -> &HashMap<u16, usize> {
        &self.shape_counts
    }

    /// Whether the movie defines buttons, whose records place characters
    /// this module does not read.
    #[must_use]
    pub fn defines_buttons(&self) -> bool {
        self.tags
            .iter()
            .any(|tag| matches!(tag.code, DEFINE_BUTTON | DEFINE_BUTTON_2))
    }

    /// The placements of the movie's own timeline. `None` when one is too
    /// short for its fields.
    #[must_use]
    pub fn timeline(&self) -> Option<Vec<Placement>> {
        placements(self.bytes, &self.tags)
    }

    /// The movie's sprites: an id, a frame count, then tags of their own.
    /// `None` when a sprite's tags run past its body or a placement is too
    /// short.
    #[must_use]
    pub fn sprites(&self) -> Option<Vec<Sprite>> {
        self.bodies(DEFINE_SPRITE)
            .map(|body| {
                let own = tags(body, 4..body.len())?;
                Some(Sprite {
                    id: u16_at(body, 0)?,
                    placements: placements(body, &own)?,
                })
            })
            .collect()
    }

    /// The class name the movie gives each character it exports, without
    /// its NUL: a count, then a character id and a name each. `None` when a
    /// table is cut short.
    #[must_use]
    pub fn classes(&self) -> Option<Vec<(u16, &'a [u8])>> {
        let mut found = Vec::new();
        for body in self.bodies(SYMBOL_CLASS) {
            let mut rest = body.get(2..)?;
            for _ in 0..u16_at(body, 0)? {
                let id = u16_at(rest, 0)?;
                let named = rest.get(2..)?;
                let end = named.iter().position(|&byte| byte == 0)?;
                found.push((id, named.get(..end)?));
                rest = named.get(end + 1..)?;
            }
        }
        Some(found)
    }

    /// A copy of the movie in which each body replaces the shape it names,
    /// and every other tag is as it was. `None` when two bodies name one
    /// shape, or a body names no `DefineShape3` or more than one.
    #[must_use]
    pub fn replace_shapes(&self, bodies: &[ShapeBody]) -> Option<Vec<u8>> {
        let counts = &self.shape_counts;
        let mut by_id = HashMap::with_capacity(bodies.len());
        for body in bodies {
            if counts.get(&body.id) != Some(&1) || by_id.insert(body.id, body).is_some() {
                return None;
            }
        }
        let mut out = Vec::with_capacity(self.bytes.len());
        out.extend(self.bytes.get(..self.first)?);
        for tag in &self.tags {
            let id = (tag.code == DEFINE_SHAPE_3)
                .then(|| u16_at(self.bytes.get(tag.body.clone())?, 0))
                .flatten();
            match id.and_then(|id| by_id.get(&id)) {
                Some(body) => {
                    out.extend(((DEFINE_SHAPE_3 << 6) | LONG_TAG).to_le_bytes());
                    out.extend(u32::try_from(body.bytes.len()).ok()?.to_le_bytes());
                    out.extend(&body.bytes);
                }
                None => out.extend(self.bytes.get(tag.start..tag.body.end)?),
            }
        }
        let len = u32::try_from(out.len()).ok()?.to_le_bytes();
        out.get_mut(4..8)?.copy_from_slice(&len);
        Some(out)
    }
}

/// Bits written most significant first.
struct Bits {
    bytes: Vec<u8>,
    /// Bits still free in the last byte.
    free: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        for bit in (0..count).rev() {
            if self.free == 0 {
                self.bytes.push(0);
                self.free = 8;
            }
            self.free -= 1;
            if let Some(last) = self.bytes.last_mut() {
                *last |= u8::from((value >> bit) & 1 == 1) << self.free;
            }
        }
    }

    fn signed(&mut self, value: i32, count: u32) {
        self.put(value.cast_unsigned(), count);
    }
}

/// The bits a signed field needs to hold every value.
fn field_bits(values: &[i32]) -> u32 {
    values
        .iter()
        .map(|&value| 33 - (value ^ (value >> 31)).leading_zeros())
        .max()
        .unwrap_or(1)
}

fn delta(from: Point, to: Point) -> Option<[i32; 2]> {
    Some([to.x.checked_sub(from.x)?, to.y.checked_sub(from.y)?])
}

/// A `DefineShape3` body: `contours` filled with one solid colour, holes
/// running the other way to the outer contours. Every edge carries the
/// colour as fill style 0. `None` when a contour does not end where it
/// starts, or a coordinate does not fit the record's fields.
#[must_use]
pub fn shape(id: u16, rgba: [u8; 4], contours: &[Contour]) -> Option<ShapeBody> {
    if !contours.iter().all(Contour::closed) {
        return None;
    }
    let points = || contours.iter().flat_map(Contour::points);
    let bounds = [
        points().map(|p| p.x).min().unwrap_or(0),
        points().map(|p| p.x).max().unwrap_or(0),
        points().map(|p| p.y).min().unwrap_or(0),
        points().map(|p| p.y).max().unwrap_or(0),
    ];
    let mut bits = Bits {
        bytes: id.to_le_bytes().to_vec(),
        free: 0,
    };
    let count = field_bits(&bounds);
    if count > WIDEST_FIELD_BITS {
        return None;
    }
    bits.put(count, 5);
    for value in bounds {
        bits.signed(value, count);
    }
    // Byte-aligned from here: one solid fill style, no line styles, then
    // one fill-index bit and no line-index bits.
    bits.free = 0;
    bits.bytes.extend([1, 0]);
    bits.bytes.extend(rgba);
    bits.bytes.extend([0, 0x10]);
    for (index, contour) in contours.iter().enumerate() {
        let first = index == 0;
        bits.put(0, 1);
        bits.put(
            if first {
                FILL_STYLE_0 | MOVE_TO
            } else {
                MOVE_TO
            },
            5,
        );
        let count = field_bits(&[contour.start.x, contour.start.y]);
        bits.put(count, 5);
        bits.signed(contour.start.x, count);
        bits.signed(contour.start.y, count);
        if first {
            bits.put(1, 1);
        }
        let mut at = contour.start;
        for edge in &contour.edges {
            let (line, curve);
            let (straight, deltas, to): (bool, &[i32], Point) = match *edge {
                Edge::Line(to) => {
                    line = delta(at, to)?;
                    if line == [0, 0] {
                        continue;
                    }
                    (true, &line, to)
                }
                Edge::Curve { control, to } => {
                    let ([cx, cy], [x, y]) = (delta(at, control)?, delta(control, to)?);
                    curve = [cx, cy, x, y];
                    (false, &curve, to)
                }
            };
            let count = field_bits(deltas).max(2);
            if count > WIDEST_EDGE_BITS {
                return None;
            }
            // An edge record: straight or curved, the deltas' width less
            // two, and for a straight edge its general form (both deltas).
            bits.put(if straight { 0b11 } else { 0b10 }, 2);
            bits.put(count - 2, 4);
            if straight {
                bits.put(1, 1);
            }
            for &value in deltas {
                bits.signed(value, count);
            }
            at = to;
        }
    }
    // The end record: a non-edge record with no flags.
    bits.put(0, 6);
    Some(ShapeBody {
        id,
        bytes: bits.bytes,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        Change, Contour, DEFINE_SHAPE_3, DEFINE_SPRITE, Depth, END, Edge, Movie, PLACE_OBJECT_2,
        PLACE_OBJECT_3, Placement, Point, REMOVE_OBJECT_2, SHOW_FRAME, SYMBOL_CLASS, ShapeBody,
        Sprite, Tag, names_movie, shape, tags,
    };

    fn tag(code: u16, body: &[u8]) -> Vec<u8> {
        let mut bytes = ((code << 6) | u16::try_from(body.len()).unwrap())
            .to_le_bytes()
            .to_vec();
        bytes.extend(body);
        bytes
    }

    fn long_tag(code: u16, body: &[u8]) -> Vec<u8> {
        let mut bytes = ((code << 6) | 0x3F).to_le_bytes().to_vec();
        bytes.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        bytes.extend(body);
        bytes
    }

    /// A movie: the magic, a version, the length, a frame rectangle of
    /// 11-bit fields (7 bytes), a rate and a frame count, then the tags.
    fn movie(tags: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = b"GFX\x0b\0\0\0\0".to_vec();
        bytes.extend([11 << 3, 0, 0, 0, 0, 0, 0]);
        bytes.extend([0, 30, 1, 0]);
        bytes.extend(tags.concat());
        let len = u32::try_from(bytes.len()).unwrap().to_le_bytes();
        bytes[4..8].copy_from_slice(&len);
        bytes
    }

    fn body(id: u16, bytes: &[u8]) -> ShapeBody {
        ShapeBody {
            id,
            bytes: bytes.to_vec(),
        }
    }

    fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    fn lines(start: Point, to: &[Point]) -> Contour {
        Contour {
            start,
            edges: to.iter().map(|&to| Edge::Line(to)).collect(),
        }
    }

    /// The flags of each style change record of a written shape, decoded
    /// apart from the writer: after the id, the bounds (`bounds_bytes` of
    /// them) and 8 bytes of styles, records of one fill bit and no line
    /// bits.
    fn record_flags(body: &ShapeBody, bounds_bytes: usize) -> Vec<u32> {
        let bytes = &body.bytes;
        let mut at = (2 + bounds_bytes + 8) * 8;
        let mut take = |count: usize| {
            let mut value = 0;
            for _ in 0..count {
                value = (value << 1) | u32::from(bytes[at / 8] >> (7 - at % 8) & 1);
                at += 1;
            }
            value
        };
        let mut found = Vec::new();
        loop {
            if take(1) == 0 {
                let flags = take(5);
                if flags == 0 {
                    return found;
                }
                found.push(flags);
                if flags & 1 != 0 {
                    let width = take(5) as usize;
                    take(2 * width);
                }
                take(usize::from(flags & 2 != 0) + usize::from(flags & 4 != 0));
            } else if take(1) == 1 {
                let width = take(4) as usize + 2;
                let both = take(1) == 1;
                take(if both { 2 * width } else { 1 + width });
            } else {
                let width = take(4) as usize + 2;
                take(4 * width);
            }
        }
    }

    #[test]
    fn tags_with_short_and_long_headers_are_listed_up_to_the_end_tag() {
        let movie = movie(&[
            tag(DEFINE_SPRITE, &[1, 2, 3, 4]),
            long_tag(DEFINE_SHAPE_3, &[4, 5]),
            tag(END, &[]),
            tag(DEFINE_SPRITE, &[9]),
        ]);

        let parsed = Movie::parse(&movie).unwrap();

        // The first tag starts after 8 header bytes, 7 of the rectangle and 4
        // of the rate and the frame count.
        assert_eq!(parsed.first, 19);
        assert_eq!(
            parsed.tags,
            [
                Tag {
                    code: DEFINE_SPRITE,
                    start: 19,
                    body: 21..25
                },
                Tag {
                    code: DEFINE_SHAPE_3,
                    start: 25,
                    body: 31..33
                },
                Tag {
                    code: END,
                    start: 33,
                    body: 35..35
                },
            ]
        );
    }

    #[test]
    fn a_tag_past_its_range_is_refused() {
        let bytes = tag(DEFINE_SPRITE, &[1, 2, 3]);

        assert_eq!(tags(&bytes, 0..4), None);
        assert_eq!(tags(&bytes, 0..1), None);
        let huge = [0xFF, 0x09, 0xFF, 0xFF, 0xFF, 0xFF];
        assert_eq!(tags(&huge, 0..huge.len()), None);
    }

    #[test]
    fn bytes_that_are_no_movie_are_not_parsed() {
        let good = movie(&[tag(END, &[])]);
        let mut compressed = good.clone();
        compressed[..3].copy_from_slice(b"CFX");
        let mut wrong_length = good.clone();
        wrong_length[4] += 1;

        assert!(Movie::parse(&good).is_some());
        assert!(Movie::parse(&compressed).is_none());
        assert!(Movie::parse(&wrong_length).is_none());
        assert!(Movie::parse(b"GFX").is_none());
        assert!(Movie::parse(&[]).is_none());
    }

    #[test]
    fn replacing_a_shape_keeps_every_other_tag_and_fixes_the_length() {
        let sprite = tag(DEFINE_SPRITE, &[1, 0, 1, 0]);
        let kept = tag(DEFINE_SHAPE_3, &[8, 0, 0xAA]);
        let original = movie(&[
            sprite.clone(),
            tag(DEFINE_SHAPE_3, &[7, 0, 0xAA]),
            kept.clone(),
            tag(END, &[]),
        ]);
        let new = [7, 0, 1, 2, 3, 4, 5];

        let replaced = Movie::parse(&original)
            .unwrap()
            .replace_shapes(&[body(7, &new)])
            .unwrap();

        let expected = movie(&[sprite, long_tag(DEFINE_SHAPE_3, &new), kept, tag(END, &[])]);
        assert_eq!(replaced, expected);
    }

    #[test]
    fn a_body_that_names_no_shape_once_is_refused() {
        let twice = movie(&[
            tag(DEFINE_SHAPE_3, &[7, 0]),
            tag(DEFINE_SHAPE_3, &[7, 0]),
            tag(END, &[]),
        ]);
        let once = movie(&[tag(DEFINE_SHAPE_3, &[7, 0]), tag(END, &[])]);

        let twice = Movie::parse(&twice).unwrap();
        let once = Movie::parse(&once).unwrap();

        assert_eq!(twice.shape_counts(), &HashMap::from([(7, 2)]));
        assert_eq!(twice.replace_shapes(&[body(7, &[7, 0])]), None);
        assert_eq!(once.replace_shapes(&[body(8, &[8, 0])]), None);
        assert_eq!(
            once.replace_shapes(&[body(7, &[7, 0, 1]), body(7, &[7, 0, 2])]),
            None
        );
    }

    #[test]
    fn a_shape_tag_too_short_for_an_id_names_no_shape() {
        // The empty shape's id would otherwise be read from the next tag's
        // header, which reads as 7.
        let movie = movie(&[tag(DEFINE_SHAPE_3, &[]), tag(END, &[0; 7])]);

        let parsed = Movie::parse(&movie).unwrap();

        assert!(parsed.shape_counts().is_empty());
        assert_eq!(parsed.replace_shapes(&[body(7, &[7, 0])]), None);
    }

    #[test]
    fn a_sprite_lists_what_each_placement_does_frame_by_frame() {
        let own = [
            // Places character 0x0102 at depth 1, with a matrix.
            tag(PLACE_OBJECT_2, &[0b110, 1, 0, 2, 1, 0]),
            // Replaces the character at depth 1 with 0x0003.
            tag(PLACE_OBJECT_2, &[0b011, 1, 0, 3, 0]),
            // Places character 9 at depth 2 as a mask.
            tag(PLACE_OBJECT_2, &[0x46, 2, 0, 9, 0]),
            tag(SHOW_FRAME, &[]),
            // Moves the character at depth 1.
            tag(PLACE_OBJECT_2, &[0b101, 1, 0, 0]),
            tag(REMOVE_OBJECT_2, &[1, 0]),
            tag(PLACE_OBJECT_3, &[0, 0, 4, 0]),
            tag(END, &[]),
        ]
        .concat();
        let sprite = [&[5, 0, 2, 0][..], &own].concat();
        let movie = movie(&[tag(DEFINE_SPRITE, &sprite), tag(END, &[])]);

        let sprites = Movie::parse(&movie).unwrap().sprites();

        let at = |frame, depth, change| Placement {
            frame,
            depth: Depth(depth),
            change,
        };
        assert_eq!(
            sprites,
            Some(vec![Sprite {
                id: 5,
                placements: vec![
                    at(0, 1, Change::Place(0x0102)),
                    at(0, 1, Change::Replace(3)),
                    at(0, 2, Change::Mask(9)),
                    at(1, 1, Change::Modify),
                    at(1, 1, Change::Remove),
                    at(1, 4, Change::Unread),
                ],
            }])
        );
    }

    #[test]
    fn a_placement_too_short_for_its_fields_is_refused() {
        // Names a character, but its body ends after the depth.
        let own = [tag(PLACE_OBJECT_2, &[0b10, 1, 0]), tag(END, &[])].concat();
        let sprite = [&[5, 0, 1, 0][..], &own].concat();
        let movie = movie(&[
            tag(DEFINE_SPRITE, &sprite),
            tag(PLACE_OBJECT_2, &[0b10, 1]),
            tag(END, &[]),
        ]);

        let parsed = Movie::parse(&movie).unwrap();

        assert_eq!(parsed.sprites(), None);
        assert_eq!(parsed.timeline(), None);
    }

    #[test]
    fn classes_are_read_name_by_name_and_a_cut_table_is_refused() {
        let table = [&[2, 0, 5, 0][..], b"first\0", &[6, 0], b"second\0"].concat();
        let good = movie(&[tag(SYMBOL_CLASS, &table), tag(END, &[])]);
        let cut = movie(&[tag(SYMBOL_CLASS, &table[..table.len() - 1]), tag(END, &[])]);

        assert_eq!(
            Movie::parse(&good).unwrap().classes(),
            Some(vec![(5, &b"first"[..]), (6, &b"second"[..])])
        );
        assert_eq!(Movie::parse(&cut).unwrap().classes(), None);
    }

    #[test]
    fn a_clockwise_triangle_is_filled_with_style_zero() {
        let triangle = lines(point(0, 0), &[point(20, 0), point(0, 20), point(0, 0)]);

        let body = shape(7, [0xFF; 4], &[triangle]).unwrap();

        assert_eq!(body.id, 7);
        assert_eq!(
            body.bytes,
            [
                0x07, 0x00, // the id
                0x30, 0x0A, 0x00, 0xA0, // bounds 0..20 by 0..20 in 6-bit fields
                0x01, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x00,
                0x10, // one solid fill, one fill bit
                0x0C, 0x27, 0x4A, 0x80, 0x69, 0xB1, 0x4D, 0x20, 0x58,
                0x00, // move, three lines, end
            ]
        );
    }

    #[test]
    fn a_curve_is_written_as_a_control_and_an_anchor_delta() {
        let leaf = Contour {
            start: point(10, 10),
            edges: vec![
                Edge::Curve {
                    control: point(30, 10),
                    to: point(30, 30),
                },
                Edge::Line(point(10, 10)),
            ],
        };

        let body = shape(1, [0, 0, 0, 0xFF], &[leaf]).unwrap();

        assert_eq!(
            body.bytes,
            [
                0x01, 0x00, // the id
                0x31, 0x4F, 0x14, 0xF0, // bounds 10..30 by 10..30 in 6-bit fields
                0x01, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00,
                0x10, // one solid fill, one fill bit
                0x0C, 0xAA, 0x56, 0x45, 0x00, 0x01, 0x4D, 0x36, 0x58,
                0x00, // move, a curve, a line, end
            ]
        );
    }

    #[test]
    fn only_the_first_contour_sets_the_fill_and_every_contour_moves() {
        let triangle = lines(point(0, 0), &[point(0, 20), point(20, 0), point(0, 0)]);
        let outer = lines(
            point(0, 0),
            &[point(0, 40), point(40, 40), point(40, 0), point(0, 0)],
        );
        let hole = lines(
            point(10, 10),
            &[point(30, 10), point(30, 30), point(10, 30), point(10, 10)],
        );

        let triangle = shape(1, [0; 4], &[triangle]).unwrap();
        let ring = shape(2, [0; 4], &[hole, outer]).unwrap();

        // Bounds of 6-bit fields take 4 bytes, of 7-bit fields 5.
        assert_eq!(record_flags(&triangle, 4), [0b00011]);
        assert_eq!(record_flags(&ring, 5), [0b00011, 0b00001]);
    }

    #[test]
    fn a_movie_with_a_button_says_so() {
        let plain = movie(&[tag(END, &[])]);
        let with_button = movie(&[tag(34, &[1, 0, 0, 0, 0, 0]), tag(END, &[])]);

        assert!(!Movie::parse(&plain).unwrap().defines_buttons());
        assert!(Movie::parse(&with_button).unwrap().defines_buttons());
    }

    #[test]
    fn an_outline_moves_into_other_points() {
        let contour = lines(point(1, 2), &[point(3, 4), point(1, 2)]);

        let moved = contour.map(|p| [f64::from(p.x) / 2.0, f64::from(p.y)]);

        assert_eq!(moved.start, [0.5, 2.0]);
        assert_eq!(
            moved.edges,
            [Edge::Line([1.5, 4.0]), Edge::Line([0.5, 2.0])]
        );
        assert_eq!(
            moved.points().collect::<Vec<_>>(),
            [[0.5, 2.0], [1.5, 4.0], [1.5, 4.0], [0.5, 2.0], [0.5, 2.0]]
        );
    }

    #[test]
    fn a_shape_without_contours_is_empty() {
        let body = shape(0x0102, [0, 0, 0, 0xFF], &[]).unwrap();

        assert_eq!(
            body.bytes,
            [
                0x02, 0x01, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x10, 0x00
            ]
        );
    }

    #[test]
    fn an_edge_too_long_for_its_record_is_refused() {
        let long = lines(point(0, 0), &[point(1 << 16, 0), point(0, 0)]);
        let far = lines(
            point(i32::MAX, 0),
            &[point(i32::MIN, 0), point(i32::MAX, 0)],
        );

        assert_eq!(shape(1, [0; 4], &[long]), None);
        assert_eq!(shape(1, [0; 4], &[far]), None);
    }

    #[test]
    fn a_contour_that_does_not_end_where_it_starts_is_refused() {
        let open = lines(point(0, 0), &[point(20, 0), point(0, 20)]);
        let empty = lines(point(0, 0), &[]);

        assert_eq!(shape(1, [0; 4], &[open]), None);
        assert_eq!(shape(1, [0; 4], &[empty]), None);
    }

    #[test]
    fn a_header_names_a_movie_of_its_own_length() {
        let header = *b"GFX\x0b\x20\0\0\0";

        assert!(names_movie(&header, 32));
        assert!(!names_movie(&header, 31));
        assert!(!names_movie(b"CFX\x0b\x20\0\0\0", 32));
    }
}
