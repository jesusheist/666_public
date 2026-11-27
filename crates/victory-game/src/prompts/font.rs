//! Glyph outlines from the host's fonts, for the key caps' labels. It reads
//! outlines through GDI and draws nothing.

use std::ptr;

use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateFontW, DEFAULT_CHARSET, DeleteDC, DeleteObject, FIXED, FW_BOLD,
    GGI_MARK_NONEXISTING_GLYPHS, GGO_NATIVE, GGO_UNHINTED, GLYPHMETRICS, GetGlyphIndicesW,
    GetGlyphOutlineW, HDC, HFONT, MAT2, SelectObject, TT_PRIM_LINE, TT_PRIM_QSPLINE,
};

use victory_formats::gfx::{Contour, Edge};

use super::{EmPoint, Glyph};

/// A face of the host the labels are drawn with; both ship with Windows.
#[derive(Clone, Copy, Debug)]
enum Face {
    /// Segoe UI in bold, for letters and arrows: heavy, like the letters of
    /// the title's own button pictures.
    Letters,
    /// Segoe UI Symbol, for the signs of Enter and Backspace, which the
    /// first face lacks.
    Signs,
}

impl Face {
    fn name(self) -> &'static str {
        match self {
            Self::Letters => "Segoe UI",
            Self::Signs => "Segoe UI Symbol",
        }
    }
}

/// The faces labels are drawn with, each opened when the host has it.
pub(super) struct Fonts {
    letters: Option<Font>,
    signs: Option<Font>,
}

impl Fonts {
    pub(super) fn open() -> Self {
        Self {
            letters: Font::open(Face::Letters),
            signs: Font::open(Face::Signs),
        }
    }

    /// The character's outline from the first face that has it.
    pub(super) fn glyph(&self, character: char) -> Option<Glyph> {
        let from = |font: &Option<Font>| font.as_ref()?.glyph(character);
        from(&self.letters).or_else(|| from(&self.signs))
    }
}
/// The size outlines are taken at.
const EM_PIXELS: i32 = 1024;
/// The largest outline taken.
const LARGEST_OUTLINE_BYTES: u32 = 1 << 20;
/// The glyph index GDI gives a character the font lacks.
const MISSING_GLYPH: u16 = 0xFFFF;
/// What `GetGlyphOutlineW` returns on failure (`GDI_ERROR`).
const OUTLINE_FAILED: u32 = u32::MAX;

/// A host face, selected into a memory device context.
struct Font {
    dc: HDC,
    font: HFONT,
}

impl Font {
    fn open(face: Face) -> Option<Self> {
        let face: Vec<u16> = face.name().encode_utf16().chain([0]).collect();
        // SAFETY: plain GDI calls; `face` is NUL-terminated and outlives the
        // call that reads it.
        unsafe {
            let dc = CreateCompatibleDC(ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let font = CreateFontW(
                -EM_PIXELS,
                0,
                0,
                0,
                FW_BOLD.cast_signed(),
                0,
                0,
                0,
                u32::from(DEFAULT_CHARSET),
                0,
                0,
                0,
                0,
                face.as_ptr(),
            );
            if font.is_null() {
                DeleteDC(dc);
                return None;
            }
            SelectObject(dc, font);
            Some(Self { dc, font })
        }
    }

    /// The character's outline; `None` when the font has no glyph for it.
    fn glyph(&self, character: char) -> Option<Glyph> {
        let mut units = [0u16; 2];
        let [unit] = *character.encode_utf16(&mut units) else {
            return None;
        };
        let one = FIXED { fract: 0, value: 1 };
        let zero = FIXED { fract: 0, value: 0 };
        let identity = MAT2 {
            eM11: one,
            eM12: zero,
            eM21: zero,
            eM22: one,
        };
        let format = GGO_NATIVE | GGO_UNHINTED;
        let mut index = MISSING_GLYPH;
        let mut metrics = GLYPHMETRICS::default();
        // SAFETY: one UTF-16 unit in and one index out; the outline call
        // with no buffer only measures.
        let size = unsafe {
            GetGlyphIndicesW(
                self.dc,
                &raw const unit,
                1,
                &raw mut index,
                GGI_MARK_NONEXISTING_GLYPHS,
            );
            if index == MISSING_GLYPH {
                return None;
            }
            GetGlyphOutlineW(
                self.dc,
                u32::from(unit),
                format,
                &raw mut metrics,
                0,
                ptr::null_mut(),
                &raw const identity,
            )
        };
        if size == OUTLINE_FAILED || size > LARGEST_OUTLINE_BYTES {
            return None;
        }
        let mut bytes = vec![0u8; usize::try_from(size).ok()?];
        // SAFETY: the buffer is `size` bytes, the size the call just asked
        // for.
        let written = unsafe {
            GetGlyphOutlineW(
                self.dc,
                u32::from(unit),
                format,
                &raw mut metrics,
                size,
                bytes.as_mut_ptr().cast(),
                &raw const identity,
            )
        };
        if written != size {
            return None;
        }
        Some(Glyph {
            contours: contours(&bytes)?,
            advance_ems: f64::from(metrics.gmCellIncX) / f64::from(EM_PIXELS),
        })
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: both handles are ours and not used after this.
        unsafe {
            DeleteDC(self.dc);
            DeleteObject(self.font);
        }
    }
}

/// The contours of a native glyph outline: polygon records (a size, a type
/// and a start point) of curve records (a kind, a count and that many
/// points), every point a pair of 16.16 fixed-point pixels.
fn contours(bytes: &[u8]) -> Option<Vec<Contour<EmPoint>>> {
    let word = |at: usize| Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
    let long = |at: usize| Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?));
    let point = |at: usize| -> Option<EmPoint> {
        let fixed =
            |at: usize| Some(f64::from(long(at)?.cast_signed()) / 65536.0 / f64::from(EM_PIXELS));
        Some([fixed(at)?, fixed(at.checked_add(4)?)?])
    };
    let mut found = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let size = usize::try_from(long(at)?).ok()?;
        let end = at
            .checked_add(size)
            .filter(|&end| size >= 16 && end <= bytes.len())?;
        let start = point(at + 8)?;
        let mut edges = Vec::new();
        let mut from = start;
        let mut record = at + 16;
        while record < end {
            let (kind, count) = (u32::from(word(record)?), usize::from(word(record + 2)?));
            let next = record + 4 + count * 8;
            if next > end {
                return None;
            }
            let points = (0..count)
                .map(|i| point(record + 4 + i * 8))
                .collect::<Option<Vec<_>>>()?;
            match kind {
                TT_PRIM_LINE => edges.extend(points.iter().map(|&to| Edge::Line(to))),
                // A spline's points are controls, each curve ending midway
                // to the next control, and the last point is the last end.
                TT_PRIM_QSPLINE => edges.extend(points.windows(2).enumerate().map(|(i, pair)| {
                    let (control, next) = (pair[0], pair[1]);
                    let to = if i + 2 == points.len() {
                        next
                    } else {
                        [(control[0] + next[0]) / 2.0, (control[1] + next[1]) / 2.0]
                    };
                    Edge::Curve { control, to }
                })),
                _ => return None,
            }
            from = edges
                .last()
                .map_or(from, |edge: &Edge<EmPoint>| edge.points()[1]);
            record = next;
        }
        if from != start {
            edges.push(Edge::Line(start));
        }
        found.push(Contour { start, edges });
        at = end;
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::super::pad::classic_key;
    use super::super::{ICONS, key_label};
    use victory_formats::gfx::{Contour, Edge};

    use super::{EM_PIXELS, Fonts, contours};

    #[test]
    fn the_host_font_outlines_every_label_character_within_its_advance() {
        let fonts = Fonts::open();
        let characters: Vec<char> = ICONS
            .iter()
            .flat_map(|(_, controls)| controls.iter())
            .filter_map(|&control| key_label(classic_key(control)))
            .flat_map(str::chars)
            .collect();

        for character in characters {
            let glyph = fonts
                .glyph(character)
                .unwrap_or_else(|| panic!("{character}"));
            let advance = glyph.advance_ems;
            assert!(advance > 0.1 && advance < 1.5, "{character} {advance}");
            assert!(!glyph.contours.is_empty(), "{character}");
            // Ems, not the font's design units: the outline stays within
            // the pen's advance and about an em of height.
            for contour in &glyph.contours {
                for [x, y] in contour.points() {
                    assert!((-0.1..=advance + 0.1).contains(&x), "{character} x {x}");
                    assert!((-0.4..=1.2).contains(&y), "{character} y {y}");
                }
            }
        }
    }

    fn fixed(pixels: i32) -> [u8; 4] {
        (pixels << 16).to_le_bytes()
    }

    fn point(x: i32, y: i32) -> Vec<u8> {
        [fixed(x), fixed(y)].concat()
    }

    #[test]
    fn a_polygon_of_a_line_and_a_spline_is_closed_back_to_its_start() {
        let em = EM_PIXELS;
        let mut bytes = Vec::new();
        // One polygon: 16 header bytes, a line record of one point and a
        // spline record of three points.
        bytes.extend(56u32.to_le_bytes());
        bytes.extend(24u32.to_le_bytes());
        bytes.extend(point(0, 0));
        bytes.extend([1, 0, 1, 0]);
        bytes.extend(point(em, 0));
        bytes.extend([2, 0, 3, 0]);
        bytes.extend(point(em, em));
        bytes.extend(point(0, em));
        bytes.extend(point(0, em / 2));

        assert_eq!(
            contours(&bytes),
            Some(vec![Contour {
                start: [0.0, 0.0],
                edges: vec![
                    Edge::Line([1.0, 0.0]),
                    Edge::Curve {
                        control: [1.0, 1.0],
                        to: [0.5, 1.0]
                    },
                    Edge::Curve {
                        control: [0.0, 1.0],
                        to: [0.0, 0.5]
                    },
                    Edge::Line([0.0, 0.0]),
                ],
            }])
        );
    }

    #[test]
    fn a_record_past_the_buffer_is_refused() {
        let mut bytes = Vec::new();
        bytes.extend(64u32.to_le_bytes());
        bytes.extend(24u32.to_le_bytes());
        bytes.extend(point(0, 0));

        assert_eq!(contours(&bytes), None);
    }

    #[test]
    fn a_curve_count_past_its_polygon_is_refused() {
        // The first polygon's line names two points and holds one; the
        // second point would be the next polygon's header.
        let mut bytes = Vec::new();
        bytes.extend(28u32.to_le_bytes());
        bytes.extend(24u32.to_le_bytes());
        bytes.extend(point(0, 0));
        bytes.extend([1, 0, 2, 0]);
        bytes.extend(point(1, 1));
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(24u32.to_le_bytes());
        bytes.extend(point(0, 0));

        assert_eq!(contours(&bytes), None);
    }
}
