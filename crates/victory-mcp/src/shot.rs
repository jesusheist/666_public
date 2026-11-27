//! Screenshots: rows read back from the swap chain's frame become an 8-bit
//! sRGB PNG.
//!
//! What the pixels mean follows the swap chain, which sets no colour space:
//! 8-bit and 10-bit formats are display-encoded already (10 bits are cut to
//! 8), 16-bit floats are linear (scRGB) and get the sRGB curve, with values
//! above 1.0 clipped.

use std::path::{Path, PathBuf};

/// DXGI_FORMAT values the swap chain's frame can have.
pub mod format {
    pub const R16G16B16A16_TYPELESS: u32 = 9;
    pub const R16G16B16A16_FLOAT: u32 = 10;
    pub const R16G16B16A16_UNORM: u32 = 11;
    pub const R16G16B16A16_UINT: u32 = 12;
    pub const R16G16B16A16_SNORM: u32 = 13;
    pub const R10G10B10A2_TYPELESS: u32 = 23;
    pub const R10G10B10A2_UNORM: u32 = 24;
    pub const R10G10B10A2_UINT: u32 = 25;
    pub const R8G8B8A8_TYPELESS: u32 = 27;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const R8G8B8A8_UINT: u32 = 30;
    pub const R8G8B8A8_SNORM: u32 = 31;
    pub const B8G8R8A8_UNORM: u32 = 87;
    pub const B8G8R8X8_UNORM: u32 = 88;
    pub const R10G10B10_XR_BIAS_A2_UNORM: u32 = 89;
    pub const B8G8R8A8_TYPELESS: u32 = 90;
    pub const B8G8R8A8_UNORM_SRGB: u32 = 91;
    pub const B8G8R8X8_TYPELESS: u32 = 92;
    pub const B8G8R8X8_UNORM_SRGB: u32 = 93;
}

/// Bytes of one pixel in `format`; `None` for a format not converted.
pub fn bytes_per_pixel(format: u32) -> Option<usize> {
    use format::*;
    match format {
        R16G16B16A16_TYPELESS..=R16G16B16A16_SNORM => Some(8),
        R10G10B10A2_TYPELESS..=R10G10B10A2_UINT
        | R8G8B8A8_TYPELESS..=R8G8B8A8_SNORM
        | B8G8R8A8_UNORM
        | B8G8R8X8_UNORM
        | R10G10B10_XR_BIAS_A2_UNORM
        | B8G8R8A8_TYPELESS..=B8G8R8X8_UNORM_SRGB => Some(4),
        _ => None,
    }
}

/// A frame read back: `rows` hold `height` rows of `width` pixels with no
/// padding between rows.
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    /// A DXGI_FORMAT value.
    pub format: u32,
    pub frame: u64,
    pub rows: Vec<u8>,
}

/// A half-precision float as an `f32`.
pub fn half_to_f32(h: u16) -> f32 {
    let (sign, exp, man) = (u32::from(h >> 15), i32::from((h >> 10) & 0x1f), f32::from(h & 0x3ff));
    let v = match exp {
        0 => man * 2f32.powi(-24),
        31 => {
            if man == 0.0 { f32::INFINITY } else { f32::NAN }
        }
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    };
    if sign == 1 { -v } else { v }
}

/// A linear value 0..1 as an 8-bit sRGB value (NaN is black, above 1 is white).
pub fn linear_to_srgb8(v: f32) -> u8 {
    if v.is_nan() {
        return 0;
    }
    let v = v.clamp(0.0, 1.0);
    let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (e * 255.0 + 0.5) as u8
}

fn unorm(v: f32) -> u8 {
    if v.is_nan() { 0 } else { (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8 }
}

/// The frame as 8-bit RGB, three bytes a pixel.
pub fn to_rgb8(frame: &RawFrame) -> Result<Vec<u8>, String> {
    use format::*;
    let bpp = bytes_per_pixel(frame.format).ok_or_else(|| format!("pixel format {} is not converted", frame.format))?;
    let pixels = frame.width as usize * frame.height as usize;
    if frame.rows.len() < pixels * bpp {
        return Err(format!("{} bytes for {pixels} pixels of {bpp} bytes", frame.rows.len()));
    }
    let src = &frame.rows[..pixels * bpp];
    let mut out = Vec::with_capacity(pixels * 3);
    match frame.format {
        R8G8B8A8_TYPELESS..=R8G8B8A8_SNORM => {
            for p in src.chunks_exact(4) {
                out.extend_from_slice(&p[..3]);
            }
        }
        B8G8R8A8_UNORM | B8G8R8X8_UNORM | B8G8R8A8_TYPELESS..=B8G8R8X8_UNORM_SRGB => {
            for p in src.chunks_exact(4) {
                out.extend_from_slice(&[p[2], p[1], p[0]]);
            }
        }
        R10G10B10A2_TYPELESS..=R10G10B10A2_UINT => {
            for p in src.chunks_exact(4) {
                let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                for shift in [0, 10, 20] {
                    let c = (v >> shift) & 0x3ff;
                    out.push(((c * 255 + 511) / 1023) as u8);
                }
            }
        }
        R10G10B10_XR_BIAS_A2_UNORM => {
            // Channel values 0..1023 stand for -0.75..1.25.
            for p in src.chunks_exact(4) {
                let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                for shift in [0, 10, 20] {
                    let c = ((v >> shift) & 0x3ff) as f32;
                    out.push(unorm((c - 384.0) / 510.0));
                }
            }
        }
        R16G16B16A16_TYPELESS | R16G16B16A16_FLOAT => {
            for p in src.chunks_exact(8) {
                for i in 0..3 {
                    out.push(linear_to_srgb8(half_to_f32(u16::from_le_bytes([p[2 * i], p[2 * i + 1]]))));
                }
            }
        }
        R16G16B16A16_UNORM | R16G16B16A16_UINT => {
            for p in src.chunks_exact(8) {
                for i in 0..3 {
                    out.push(unorm(f32::from(u16::from_le_bytes([p[2 * i], p[2 * i + 1]])) / 65535.0));
                }
            }
        }
        R16G16B16A16_SNORM => {
            for p in src.chunks_exact(8) {
                for i in 0..3 {
                    out.push(unorm(f32::from(i16::from_le_bytes([p[2 * i], p[2 * i + 1]])) / 32767.0));
                }
            }
        }
        _ => return Err(format!("pixel format {} is not converted", frame.format)),
    }
    Ok(out)
}

/// RGB reduced by `scale` (1, 2 or 4) in each direction, each new pixel the
/// mean of the block it covers (blocks at the edges are smaller).
pub fn downscale(rgb: Vec<u8>, width: u32, height: u32, scale: u32) -> (Vec<u8>, u32, u32) {
    if scale <= 1 {
        return (rgb, width, height);
    }
    let (w, h) = (width.div_ceil(scale).max(1), height.div_ceil(scale).max(1));
    let mut out = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 3];
            let mut n = 0;
            for yy in (y * scale)..((y + 1) * scale).min(height) {
                for xx in (x * scale)..((x + 1) * scale).min(width) {
                    let at = (yy as usize * width as usize + xx as usize) * 3;
                    for c in 0..3 {
                        sum[c] += u32::from(rgb[at + c]);
                    }
                    n += 1;
                }
            }
            for s in sum {
                out.push(((s + n / 2) / n) as u8);
            }
        }
    }
    (out, w, h)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend((data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend(kind);
    png.extend(data);
    let crc = crc32(&png[start..]);
    png.extend(crc.to_be_bytes());
}

/// A PNG of 8-bit RGB pixels.
pub fn encode_png(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, String> {
    let row = width as usize * 3;
    if rgb.len() != row * height as usize {
        return Err("pixel data does not match the size".into());
    }
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgb.chunks_exact(row.max(1)) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]); // 8 bits, RGB, no interlace
    chunk(&mut png, b"IHDR", &header);
    let mut compressed = vec![0u8; zlib_rs::compress_bound(raw.len())];
    let (idat, rc) = zlib_rs::compress_slice(&mut compressed, &raw, zlib_rs::DeflateConfig::new(1));
    if rc != zlib_rs::ReturnCode::Ok {
        return Err(format!("compression failed: {rc:?}"));
    }
    chunk(&mut png, b"IDAT", idat);
    chunk(&mut png, b"IEND", &[]);
    Ok(png)
}

/// Local date and time as `YYYYMMDD-HHMMSS`.
pub fn timestamp() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        // SAFETY: a plain query that fills the struct.
        let t = unsafe {
            let mut t = std::mem::zeroed::<SYSTEMTIME>();
            GetLocalTime(&mut t);
            t
        };
        format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let (y, m, d) = civil(secs / 86400);
        let s = secs % 86400;
        format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60)
    }
}

/// Year, month, day of a day count since 1970-01-01 (UTC).
#[cfg(any(not(windows), test))]
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + u64::from(m <= 2), m, d)
}

/// What a saved screenshot is.
#[derive(Debug)]
pub struct Saved {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

/// Converts, reduces and writes the frame as `<dir>\<timestamp>-f<frame>.png`.
pub fn save(frame: RawFrame, scale: u32, dir: &Path) -> Result<Saved, String> {
    let (format, number) = (frame.format, frame.frame);
    let (w, h) = (frame.width, frame.height);
    let rgb = to_rgb8(&frame)?;
    drop(frame);
    let (rgb, w, h) = downscale(rgb, w, h, scale);
    let png = encode_png(w, h, &rgb)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let mut path = dir.join(format!("{}-f{number}.png", timestamp()));
    // Two shots in one second of one frame: keep both.
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{}-f{number}-{n}.png", timestamp()));
        n += 1;
    }
    std::fs::write(&path, &png).map_err(|e| format!("cannot write {}: {e} (pixel format {format})", path.display()))?;
    Ok(Saved { path, width: w, height: h, bytes: png.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(format: u32, width: u32, height: u32, rows: Vec<u8>) -> RawFrame {
        RawFrame { width, height, format, frame: 5, rows }
    }

    #[test]
    fn eight_bit_formats_swap_or_copy() {
        let rgba = to_rgb8(&frame(format::R8G8B8A8_UNORM, 2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255])).unwrap();
        assert_eq!(rgba, vec![1, 2, 3, 4, 5, 6]);
        let bgra = to_rgb8(&frame(format::B8G8R8A8_UNORM, 1, 1, vec![3, 2, 1, 255])).unwrap();
        assert_eq!(bgra, vec![1, 2, 3]);
        let srgb = to_rgb8(&frame(format::B8G8R8A8_UNORM_SRGB, 1, 1, vec![30, 20, 10, 0])).unwrap();
        assert_eq!(srgb, vec![10, 20, 30]);
    }

    #[test]
    fn ten_bit_is_cut_to_eight() {
        let pack = |r: u32, g: u32, b: u32| (r | g << 10 | b << 20 | 3 << 30).to_le_bytes().to_vec();
        let out = to_rgb8(&frame(format::R10G10B10A2_UNORM, 2, 1, [pack(0, 0, 0), pack(1023, 512, 1)].concat())).unwrap();
        assert_eq!(out, vec![0, 0, 0, 255, 128, 0]);
        // XR bias: 384 is 0.0, 894 is 1.0.
        let out = to_rgb8(&frame(format::R10G10B10_XR_BIAS_A2_UNORM, 1, 1, pack(384, 894, 100))).unwrap();
        assert_eq!(out, vec![0, 255, 0]);
    }

    #[test]
    fn half_floats_decode() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0), 0.0);
        assert!(half_to_f32(0x7c00).is_infinite());
        assert!(half_to_f32(0x7e00).is_nan());
        assert!(half_to_f32(0x0001) > 0.0 && half_to_f32(0x0001) < 1e-7);
    }

    #[test]
    fn float_frames_get_the_srgb_curve() {
        assert_eq!(linear_to_srgb8(0.0), 0);
        assert_eq!(linear_to_srgb8(1.0), 255);
        assert_eq!(linear_to_srgb8(5.0), 255);
        assert_eq!(linear_to_srgb8(-1.0), 0);
        assert_eq!(linear_to_srgb8(f32::NAN), 0);
        // Linear 0.5 is sRGB 188.
        assert_eq!(linear_to_srgb8(0.5), 188);
        let px = [0x0000u16, 0x3800, 0x3c00, 0x3c00].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(to_rgb8(&frame(format::R16G16B16A16_FLOAT, 1, 1, px)).unwrap(), vec![0, 188, 255]);
    }

    #[test]
    fn sixteen_bit_integers() {
        let px = [0u16, 32768, 65535, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(to_rgb8(&frame(format::R16G16B16A16_UNORM, 1, 1, px)).unwrap(), vec![0, 128, 255]);
    }

    #[test]
    fn refuses_unknown_and_short() {
        assert!(to_rgb8(&frame(2, 1, 1, vec![0; 16])).is_err());
        assert!(to_rgb8(&frame(format::R8G8B8A8_UNORM, 2, 2, vec![0; 15])).is_err());
        assert_eq!(bytes_per_pixel(format::R16G16B16A16_FLOAT), Some(8));
        assert_eq!(bytes_per_pixel(format::B8G8R8X8_UNORM_SRGB), Some(4));
        assert_eq!(bytes_per_pixel(86), None);
    }

    #[test]
    fn downscale_averages_blocks() {
        // 3x2, scale 2: a block of 2x2, then one of 1x2.
        let rgb = vec![
            0, 0, 0, 100, 100, 100, 200, 0, 0, //
            100, 100, 100, 200, 200, 200, 0, 200, 0,
        ];
        let (out, w, h) = downscale(rgb.clone(), 3, 2, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, vec![100, 100, 100, 100, 100, 0]);
        assert_eq!(downscale(rgb.clone(), 3, 2, 1).0, rgb);
        let (_, w, h) = downscale(vec![0; 5 * 5 * 3], 5, 5, 4);
        assert_eq!((w, h), (2, 2));
    }

    #[test]
    fn png_has_the_right_structure_and_inflates_back() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        let rgb: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8).collect();
        let png = encode_png(4, 3, &rgb).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 3);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // The IDAT inflates to the rows behind their filter bytes.
        let len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let mut raw = vec![0u8; 3 * (4 * 3 + 1)];
        let (out, rc) = zlib_rs::decompress_slice(&mut raw, &png[41..41 + len], zlib_rs::InflateConfig::default());
        assert_eq!(rc, zlib_rs::ReturnCode::Ok);
        assert_eq!(out.len(), 39);
        assert_eq!(&out[1..13], &rgb[..12]);
        assert!(encode_png(2, 2, &[0; 5]).is_err());
    }

    #[test]
    fn dates_come_out_right() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_454), (2026, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        let t = timestamp();
        assert_eq!(t.len(), 15);
        assert_eq!(t.as_bytes()[8], b'-');
    }

    #[test]
    fn saves_a_file_named_by_frame() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("shots");
        let saved = save(frame(format::B8G8R8A8_UNORM, 4, 4, vec![9; 64]), 2, &sub).unwrap();
        assert_eq!((saved.width, saved.height), (2, 2));
        assert!(saved.path.file_name().unwrap().to_string_lossy().ends_with("-f5.png"));
        assert_eq!(std::fs::metadata(&saved.path).unwrap().len() as usize, saved.bytes);
        let again = save(frame(format::B8G8R8A8_UNORM, 4, 4, vec![9; 64]), 1, &sub).unwrap();
        assert_ne!(again.path, saved.path);
    }
}
