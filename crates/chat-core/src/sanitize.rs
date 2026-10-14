//! Validation of content from other people (and metadata removal from our own photos).
//!
//! Everything here is pure (no I/O, no state) so it can be unit tested and fuzzed
//! (see `crates/chat-core/fuzz`).

use std::io::Cursor;

use image::metadata::Orientation;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits};

// ---------------------------------------------------------------------------
// Display strings
// ---------------------------------------------------------------------------

/// Characters that change how surrounding text is displayed without being visible
/// themselves: bidi overrides/isolates/marks ("invoice<U+202E>fdp.exe" shows as
/// "invoiceexe.pdf"), zero-width and blank-looking characters (impersonation with names
/// that look empty or identical).
/// ZWJ / ZWNJ stay: emoji sequences and several scripts need them.
pub fn is_invisible_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{1D173}'..='\u{1D17A}'
    )
}

/// A name or title chosen by someone else, cleaned for display: no control or invisible
/// formatting characters, trimmed, at most `max_chars` characters.
pub fn clean_display_text(s: &str, max_chars: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !is_invisible_format_char(*c))
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Cuts `s` to at most `max_chars` characters (on a character boundary).
pub fn truncate_chars(s: &mut String, max_chars: usize) {
    if let Some((i, _)) = s.char_indices().nth(max_chars) {
        s.truncate(i);
    }
}

// ---------------------------------------------------------------------------
// Reactions
// ---------------------------------------------------------------------------

/// Longest reaction accepted (one emoji; the longest real ZWJ sequences are ~35 bytes,
/// flags of subdivisions with tags are 28).
pub const MAX_REACTION_BYTES: usize = 32;

fn is_emoji_base(c: char) -> bool {
    matches!(c as u32,
        0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x3030 | 0x303D | 0x3297 | 0x3299
        | 0x2194..=0x21AA | 0x231A..=0x23FF | 0x24C2 | 0x25A0..=0x25FF | 0x2600..=0x27BF
        | 0x2934 | 0x2935 | 0x2B00..=0x2BFF | 0x1F000..=0x1F1E5 | 0x1F200..=0x1FAFF)
}

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Whether `s` is a single emoji (one grapheme: a ZWJ sequence, a flag, a keycap, with
/// optional skin tone / variation selector / tag sequence), at most `MAX_REACTION_BYTES`.
pub fn is_single_emoji(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_REACTION_BYTES {
        return false;
    }
    let chars: Vec<char> = s.chars().collect();
    // Flags: exactly two regional indicators.
    if chars.iter().any(|c| is_regional_indicator(*c)) {
        return chars.len() == 2 && chars.iter().all(|c| is_regional_indicator(*c));
    }
    // Keycaps: [0-9#*] FE0F? 20E3
    if chars.first().is_some_and(|c| c.is_ascii_digit() || *c == '#' || *c == '*') {
        return matches!(chars.as_slice(), [_, '\u{20E3}'] | [_, '\u{FE0F}', '\u{20E3}']);
    }
    s.split('\u{200D}').all(|part| {
        let mut it = part.chars().peekable();
        let Some(base) = it.next() else { return false };
        if !is_emoji_base(base) {
            return false;
        }
        let mut tags = false;
        for c in it {
            match c {
                '\u{FE0E}' | '\u{FE0F}' | '\u{20E3}' | '\u{1F3FB}'..='\u{1F3FF}' if !tags => {}
                '\u{E0020}'..='\u{E007E}' => tags = true,
                '\u{E007F}' if tags => tags = false,
                _ => return false,
            }
        }
        !tags
    })
}

// ---------------------------------------------------------------------------
// Incoming images: real type and size before anything decodes them
// ---------------------------------------------------------------------------

/// Largest side of an incoming image the apps may decode.
pub const MAX_INCOMING_SIDE: u32 = 12_000;
/// Most pixels of an incoming image (≈200 MB once decoded to RGBA).
pub const MAX_INCOMING_PIXELS: u64 = 50_000_000;
/// Largest side of an incoming thumbnail (ours are 48 px).
pub const MAX_THUMBNAIL_SIDE: u32 = 512;

/// The image type of `b` by its magic bytes, among the formats the apps render inline
/// (jpeg, png, gif, webp, heic/heif, avif). Anything else (svg, bmp, tiff, ...) is None.
pub fn sniff_image_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.len() >= 12 && &b[4..8] == b"ftyp" {
        match &b[8..12] {
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1" => Some("image/heic"),
            b"avif" | b"avis" => Some("image/avif"),
            _ => None,
        }
    } else {
        None
    }
}

/// Whether a declared mime type is one the apps may render inline.
pub fn is_allowed_image_mime(mime: &str) -> bool {
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "image/jpeg" | "image/jpg" | "image/png" | "image/gif" | "image/webp" | "image/heic" | "image/heif" | "image/avif"
    )
}

/// Largest image dimensions announced in the `ispe` boxes of a HEIF/AVIF file (the image
/// crate cannot decode those; the platform decoders of the apps can).
fn heif_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let scan = &b[..b.len().min(1 << 20)];
    let mut best: Option<(u32, u32)> = None;
    let mut i = 4;
    while i + 16 <= scan.len() {
        if &scan[i..i + 4] == b"ispe" {
            let w = u32::from_be_bytes(scan[i + 8..i + 12].try_into().ok()?);
            let h = u32::from_be_bytes(scan[i + 12..i + 16].try_into().ok()?);
            if (w as u64) * (h as u64) > best.map(|(a, b)| a as u64 * b as u64).unwrap_or(0) {
                best = Some((w, h));
            }
            i += 16;
        } else {
            i += 1;
        }
    }
    best
}

/// Reads only the header of an incoming image: returns its real mime type and dimensions
/// when it is an allowed format within the size limits, None otherwise (then it must not be
/// shown inline).
pub fn inspect_image(b: &[u8], max_side: u32, max_pixels: u64) -> Option<(&'static str, u32, u32)> {
    let mime = sniff_image_mime(b)?;
    let (w, h) = match mime {
        "image/heic" | "image/avif" => heif_dimensions(b)?,
        _ => {
            let reader = ImageReader::new(Cursor::new(b)).with_guessed_format().ok()?;
            reader.into_dimensions().ok()?
        }
    };
    if mime == "image/gif" {
        // Animated GIF "bombs": many frames of a small canvas.
        let frames = gif_frame_count(b)? as u64;
        if frames > 10_000 || frames.saturating_mul(w as u64 * h as u64) > 20 * max_pixels {
            return None;
        }
    }
    (w > 0 && h > 0 && w <= max_side && h <= max_side && (w as u64) * (h as u64) <= max_pixels).then_some((mime, w, h))
}

/// Checks a sender's thumbnail (a `data:image/...;base64,` URL): allowed format, small.
pub fn is_safe_thumbnail(t: &str) -> bool {
    if t.len() >= 64 * 1024 {
        return false;
    }
    let Some(rest) = t.strip_prefix("data:") else { return false };
    let Some((mime, data)) = rest.split_once(";base64,") else { return false };
    if !is_allowed_image_mime(mime) {
        return false;
    }
    let Ok(bytes) = crate::crypto::unb64(data) else { return false };
    inspect_image(&bytes, MAX_THUMBNAIL_SIDE, MAX_THUMBNAIL_SIDE as u64 * MAX_THUMBNAIL_SIDE as u64)
        .is_some_and(|(m, _, _)| m != "image/heic" && m != "image/avif")
}

// ---------------------------------------------------------------------------
// Container-level metadata stripping (lossless)
// ---------------------------------------------------------------------------

/// PNG chunks kept: image data and what affects how it looks (incl. APNG animation).
/// Text (tEXt/zTXt/iTXt: XMP, comments, software), eXIf, tIME and unknown chunks go.
const PNG_KEEP: &[&[u8; 4]] = &[
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"sBIT", b"bKGD", b"pHYs",
    b"acTL", b"fcTL", b"fdAT", b"cICP", b"mDCv", b"cLLi",
];

/// Removes metadata chunks from a PNG without re-encoding. None if it is not a well-formed PNG.
pub fn strip_png_metadata(b: &[u8]) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !b.starts_with(SIG) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(SIG);
    let mut i = SIG.len();
    loop {
        let len = u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize;
        let kind: &[u8; 4] = b.get(i + 4..i + 8)?.try_into().ok()?;
        let end = i.checked_add(12)?.checked_add(len)?;
        let chunk = b.get(i..end)?;
        if PNG_KEEP.contains(&kind) {
            out.extend_from_slice(chunk);
        }
        i = end;
        if kind == b"IEND" {
            return Some(out);
        }
    }
}

/// Walks the blocks of a GIF. With `out`, writes a copy without comments and application
/// extensions other than the animation loop count (XMP, ICC and anything else). Returns the
/// number of frames, None if it is not a well-formed GIF.
fn walk_gif(b: &[u8], mut out: Option<&mut Vec<u8>>) -> Option<usize> {
    if !(b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a")) || b.len() < 13 {
        return None;
    }
    let emit = |o: &mut Option<&mut Vec<u8>>, s: &[u8]| {
        if let Some(o) = o.as_deref_mut() {
            o.extend_from_slice(s);
        }
    };
    let packed = b[10];
    let mut i = 13 + if packed & 0x80 != 0 { 3 << ((packed & 7) + 1) } else { 0 };
    emit(&mut out, b.get(..i)?);
    // Length of the sub-block chain starting at `j` (including the terminator).
    let sub_blocks = |mut j: usize| -> Option<usize> {
        let start = j;
        loop {
            let n = *b.get(j)? as usize;
            j += 1 + n;
            if n == 0 {
                return (j <= b.len()).then_some(j - start);
            }
        }
    };
    let mut frames = 0usize;
    loop {
        match *b.get(i)? {
            0x21 => {
                let label = *b.get(i + 1)?;
                let len = 2 + sub_blocks(i + 2)?;
                let keep = match label {
                    0xF9 | 0x01 => true,
                    0xFF => matches!(b.get(i + 2..i + 14), Some(id) if id == b"\x0bNETSCAPE2.0" || id == b"\x0bANIMEXTS1.0"),
                    _ => false,
                };
                if keep {
                    emit(&mut out, &b[i..i + len]);
                }
                i += len;
            }
            0x2C => {
                let desc = b.get(i..i + 10)?;
                let lct = if desc[9] & 0x80 != 0 { 3 << ((desc[9] & 7) + 1) } else { 0 };
                let data = i + 10 + lct + 1; // + LZW minimum code size
                b.get(data - 1)?;
                let len = 10 + lct + 1 + sub_blocks(data)?;
                emit(&mut out, &b[i..i + len]);
                i += len;
                frames += 1;
            }
            0x3B => {
                emit(&mut out, &[0x3B]);
                return Some(frames);
            }
            _ => return None,
        }
    }
}

pub fn strip_gif_metadata(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(b.len());
    walk_gif(b, Some(&mut out))?;
    Some(out)
}

pub fn gif_frame_count(b: &[u8]) -> Option<usize> {
    walk_gif(b, None)
}

// ---------------------------------------------------------------------------
// Outgoing images: metadata removal
// ---------------------------------------------------------------------------

/// Photos larger than this (in pixels on the longest side) are downscaled before sending.
pub const MAX_IMAGE_DIM: u32 = 2560;
const THUMB_DIM: u32 = 48;
/// PNG / GIF files up to this size keep their format (screenshots, transparency, animation).
const KEEP_LOSSLESS_MAX: usize = 8 * 1024 * 1024;

pub struct ProcessedImage {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub thumbnail: String,
}

fn thumbnail_data_url(img: &DynamicImage) -> Option<String> {
    let thumb = img.thumbnail(THUMB_DIM, THUMB_DIM).to_rgb8();
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 60);
    enc.encode_image(&thumb).ok()?;
    Some(format!("data:image/jpeg;base64,{}", crate::crypto::b64(&out)))
}

/// Decodes an image with sane limits and applies its EXIF orientation (which is about to be
/// dropped with the rest of the metadata).
fn decode_oriented(bytes: &[u8]) -> Option<(ImageFormat, DynamicImage)> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let format = reader.format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(40_000);
    limits.max_image_height = Some(40_000);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    Some((format, img))
}

fn encode_jpeg(img: &DynamicImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img.to_rgb8()).ok()?;
    Some(out)
}

fn encode_png(img: &DynamicImage) -> Option<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Prepares a photo for sending: never sends the original bytes of a photo, so EXIF (GPS
/// position, camera serial, time), XMP, comments and other metadata never leave the device.
///
/// - JPEG, WebP and other stills are decoded, rotated by their EXIF orientation, downscaled
///   to `MAX_IMAGE_DIM` and re-encoded (JPEG; PNG for transparent images up to 8 MB);
/// - PNG keeps its pixels bit for bit, without its text / EXIF chunks (up to 8 MB);
/// - GIF keeps its frames and animation, without comments and application extensions.
///
/// None when the bytes are not a decodable image (they are then sent as a plain file).
pub fn process_outgoing_image(bytes: &[u8]) -> Option<ProcessedImage> {
    let lossless = match image::guess_format(bytes).ok()? {
        ImageFormat::Png if bytes.len() <= KEEP_LOSSLESS_MAX => strip_png_metadata(bytes).map(|b| (b, "image/png")),
        ImageFormat::Gif if bytes.len() <= KEEP_LOSSLESS_MAX => strip_gif_metadata(bytes).map(|b| (b, "image/gif")),
        _ => None,
    };
    if let Some((stripped, mime)) = lossless {
        // Decoding the stripped copy also proves it is still a valid image.
        let img = image::load_from_memory(&stripped).ok()?;
        let (w, h) = img.dimensions();
        let thumbnail = thumbnail_data_url(&img)?;
        return Some(ProcessedImage { bytes: stripped, mime: mime.into(), width: w, height: h, thumbnail });
    }
    let (_, img) = decode_oriented(bytes)?;
    let (w, h) = img.dimensions();
    let img = if w.max(h) > MAX_IMAGE_DIM { img.resize(MAX_IMAGE_DIM, MAX_IMAGE_DIM, image::imageops::FilterType::Lanczos3) } else { img };
    let thumbnail = thumbnail_data_url(&img)?;
    let (out, mime) = match img.color().has_alpha().then(|| encode_png(&img)).flatten() {
        Some(png) if png.len() <= KEEP_LOSSLESS_MAX => (png, "image/png"),
        _ => (encode_jpeg(&img)?, "image/jpeg"),
    };
    let (rw, rh) = img.dimensions();
    Some(ProcessedImage { bytes: out, mime: mime.into(), width: rw, height: rh, thumbnail })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};

    /// A JPEG with an EXIF block: orientation 6 (rotate 90° clockwise) and a GPS IFD.
    fn jpeg_with_exif_gps(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, _| image::Rgb([(x * 5) as u8, 80, 160])));
        let plain = encode_jpeg(&img).unwrap();
        // TIFF (big endian): IFD0 = { Orientation = 6, GPSInfo -> GPS IFD }, GPS IFD =
        // { GPSLatitudeRef = "N", GPSMapDatum = "SECRETMARKER-GPS" }.
        let mut t: Vec<u8> = b"MM\0\x2a\0\0\0\x08".to_vec();
        t.extend_from_slice(&2u16.to_be_bytes());
        t.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]);
        let gps_ifd = 8 + 2 + 2 * 12 + 4;
        t.extend_from_slice(&[0x88, 0x25, 0, 4, 0, 0, 0, 1]);
        t.extend_from_slice(&(gps_ifd as u32).to_be_bytes());
        t.extend_from_slice(&0u32.to_be_bytes());
        let datum = b"SECRETMARKER-GPS\0";
        t.extend_from_slice(&2u16.to_be_bytes());
        t.extend_from_slice(&[0x00, 0x01, 0, 2, 0, 0, 0, 2, b'N', 0, 0, 0]);
        t.extend_from_slice(&[0x00, 0x12, 0, 2]);
        t.extend_from_slice(&(datum.len() as u32).to_be_bytes());
        t.extend_from_slice(&((gps_ifd + 2 + 2 * 12 + 4) as u32).to_be_bytes());
        t.extend_from_slice(&0u32.to_be_bytes());
        t.extend_from_slice(datum);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&t);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&plain[2..]); // skip the original SOI
        out
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn jpeg_exif_and_gps_are_removed_and_orientation_applied() {
        let src = jpeg_with_exif_gps(40, 20);
        assert!(contains(&src, b"SECRETMARKER-GPS") && contains(&src, b"Exif"));
        // The fixture really carries an orientation the decoder sees.
        let mut dec = ImageReader::new(Cursor::new(&src)).with_guessed_format().unwrap().into_decoder().unwrap();
        assert_eq!(dec.orientation().unwrap(), Orientation::Rotate90);
        let p = process_outgoing_image(&src).expect("processed");
        assert_eq!(p.mime, "image/jpeg");
        assert!(!contains(&p.bytes, b"SECRETMARKER-GPS"));
        assert!(!contains(&p.bytes, b"Exif"));
        assert!(!contains(&p.bytes, b"MM\0\x2a"));
        // Rotated so it displays upright without the (now removed) EXIF orientation.
        assert_eq!((p.width, p.height), (20, 40));
        let out = image::load_from_memory(&p.bytes).unwrap();
        assert_eq!(out.dimensions(), (20, 40));
        assert!(p.thumbnail.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn big_photos_are_downscaled() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(3000, 1000));
        let p = process_outgoing_image(&encode_jpeg(&img).unwrap()).unwrap();
        assert_eq!((p.width, p.height), (MAX_IMAGE_DIM, 853));
    }

    #[test]
    fn png_text_and_exif_chunks_are_removed_pixels_kept() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(7, 5, |x, y| image::Rgba([x as u8 * 30, y as u8 * 40, 9, 128])));
        let png = encode_png(&img).unwrap();
        // Insert tEXt and eXIf chunks after IHDR (CRCs are not checked by the stripper).
        let ihdr_end = 8 + 12 + 13;
        let mut with_meta = png[..ihdr_end].to_vec();
        for (kind, data) in [(b"tEXt", &b"Comment\0SECRETMARKER-PNG"[..]), (b"eXIf", &b"MM\0\x2aSECRETMARKER-EXIF"[..])] {
            with_meta.extend_from_slice(&(data.len() as u32).to_be_bytes());
            with_meta.extend_from_slice(kind);
            with_meta.extend_from_slice(data);
            with_meta.extend_from_slice(&[0, 0, 0, 0]);
        }
        with_meta.extend_from_slice(&png[ihdr_end..]);
        let p = process_outgoing_image(&with_meta).expect("processed");
        assert_eq!(p.mime, "image/png");
        assert!(!contains(&p.bytes, b"SECRETMARKER"));
        assert_eq!(image::load_from_memory(&p.bytes).unwrap().to_rgba8(), img.to_rgba8());
    }

    #[test]
    fn gif_comments_and_xmp_are_removed_animation_kept() {
        use image::codecs::gif::{GifEncoder, Repeat};
        let mut gif = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut gif);
            enc.set_repeat(Repeat::Infinite).unwrap();
            let frames = (0..3).map(|i| image::Frame::new(image::RgbaImage::from_pixel(4, 4, image::Rgba([i * 80, 0, 0, 255]))));
            enc.encode_frames(frames).unwrap();
        }
        assert_eq!(gif_frame_count(&gif), Some(3));
        // Comment + XMP application extension right before the trailer.
        let mut with_meta = gif[..gif.len() - 1].to_vec();
        with_meta.extend_from_slice(b"\x21\xFE\x10SECRETMARKER-GIF\x00");
        with_meta.extend_from_slice(b"\x21\xFF\x0bXMP DataXMP\x09SECRETXMP\x00");
        with_meta.push(0x3B);
        let p = process_outgoing_image(&with_meta).expect("processed");
        assert_eq!(p.mime, "image/gif");
        assert!(!contains(&p.bytes, b"SECRETMARKER") && !contains(&p.bytes, b"SECRETXMP"));
        assert!(contains(&p.bytes, b"NETSCAPE2.0"));
        assert_eq!(gif_frame_count(&p.bytes), Some(3));
    }

    #[test]
    fn sniffing_and_limits() {
        let jpeg = jpeg_with_exif_gps(30, 10);
        assert_eq!(inspect_image(&jpeg, MAX_INCOMING_SIDE, MAX_INCOMING_PIXELS), Some(("image/jpeg", 30, 10)));
        assert_eq!(inspect_image(&jpeg, 20, MAX_INCOMING_PIXELS), None);
        assert_eq!(inspect_image(&jpeg, 100, 200), None);
        assert_eq!(sniff_image_mime(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
        assert_eq!(sniff_image_mime(b"BM\0\0\0\0\0\0\0\0\0\0\0\0"), None);
        assert_eq!(sniff_image_mime(b"\0\0\0\x18ftypheic\0\0\0\0"), Some("image/heic"));
        // A PNG header claiming 100000 x 100000 pixels: rejected from the header alone.
        let mut bomb = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bomb.extend_from_slice(&100_000u32.to_be_bytes());
        bomb.extend_from_slice(&100_000u32.to_be_bytes());
        bomb.extend_from_slice(&[8, 2, 0, 0, 0]);
        let crc = crc32(&bomb[12..]);
        bomb.extend_from_slice(&crc.to_be_bytes());
        assert_eq!(inspect_image(&bomb, MAX_INCOMING_SIDE, MAX_INCOMING_PIXELS), None);
        // HEIF dimensions from its ispe box.
        let mut heic = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic".to_vec();
        heic.extend_from_slice(b"\0\0\0\x14ispe\0\0\0\0\0\0\x0f\xa0\0\0\x0b\xb8");
        assert_eq!(inspect_image(&heic, MAX_INCOMING_SIDE, MAX_INCOMING_PIXELS), Some(("image/heic", 4000, 3000)));
        heic.extend_from_slice(b"\0\0\0\x14ispe\0\0\0\0\0\x01\x86\xa0\0\0\x0b\xb8");
        assert_eq!(inspect_image(&heic, MAX_INCOMING_SIDE, MAX_INCOMING_PIXELS), None);
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn thumbnails() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(10, 10));
        let ok = thumbnail_data_url(&img).unwrap();
        assert!(is_safe_thumbnail(&ok));
        let big = DynamicImage::ImageRgb8(image::RgbImage::new(2000, 10));
        let big = format!("data:image/jpeg;base64,{}", crate::crypto::b64(&encode_jpeg(&big).unwrap()));
        assert!(!is_safe_thumbnail(&big));
        assert!(!is_safe_thumbnail("data:image/svg+xml;base64,PHN2Zy8+"));
        assert!(!is_safe_thumbnail("data:image/png;base64,AAAA"));
        assert!(!is_safe_thumbnail("javascript:alert(1)"));
    }

    #[test]
    fn display_text_loses_bidi_and_invisible_chars() {
        assert_eq!(clean_display_text("invoice\u{202E}fdp.exe", 100), "invoicefdp.exe");
        assert_eq!(clean_display_text("\u{2067}Alice\u{2069}\u{200B}\u{200F}", 100), "Alice");
        assert_eq!(clean_display_text("\u{3164}", 100), "");
        assert_eq!(clean_display_text("👨\u{200D}👩\u{200D}👧 Ana", 100), "👨\u{200D}👩\u{200D}👧 Ana");
        assert_eq!(clean_display_text("abcdef", 3), "abc");
    }

    #[test]
    fn reactions_must_be_one_emoji() {
        for ok in ["👍", "❤️", "👍🏽", "👨\u{200D}👩\u{200D}👧\u{200D}👦", "🇷🇴", "1️⃣", "#\u{20E3}", "🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}", "☺"] {
            assert!(is_single_emoji(ok), "{ok:?}");
        }
        for bad in ["", "a", "👍👍", "👍 ", "hello", "🇷", "🇷🇴🇮🇹", "1", "👍\u{202E}", "\u{200D}", "👍\u{200D}", &"👍".repeat(9)] {
            assert!(!is_single_emoji(bad), "{bad:?}");
        }
    }

    #[test]
    fn truncation_on_char_boundary() {
        let mut s = "ăîș".repeat(3);
        truncate_chars(&mut s, 4);
        assert_eq!(s, "ăîșă");
    }

    /// Property-style: random and mutated inputs never panic and never yield an image above
    /// the limits (the fuzz targets in crates/chat-core/fuzz run the same functions longer).
    #[test]
    fn decoders_survive_random_input() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xD1A106);
        let seeds: Vec<Vec<u8>> = vec![
            jpeg_with_exif_gps(16, 8),
            encode_png(&DynamicImage::ImageRgba8(image::RgbaImage::new(5, 5))).unwrap(),
            {
                let mut g = Vec::new();
                image::codecs::gif::GifEncoder::new(&mut g)
                    .encode_frame(image::Frame::new(image::RgbaImage::new(3, 3)))
                    .unwrap();
                g
            },
            crate::content::Content::Receipt { kind: crate::content::ReceiptKind::Read, ids: vec!["x".into()] }.encode(),
        ];
        for round in 0..400 {
            let mut data = seeds[round % seeds.len()].clone();
            for _ in 0..rng.random_range(1..8) {
                match rng.random_range(0..3) {
                    0 if !data.is_empty() => {
                        let i = rng.random_range(0..data.len());
                        data[i] = rng.random();
                    }
                    1 if !data.is_empty() => data.truncate(rng.random_range(0..data.len())),
                    _ => data.extend((0..rng.random_range(0..16)).map(|_| rng.random::<u8>())),
                }
            }
            if let Some((_, w, h)) = inspect_image(&data, MAX_INCOMING_SIDE, MAX_INCOMING_PIXELS) {
                assert!(w <= MAX_INCOMING_SIDE && h <= MAX_INCOMING_SIDE);
            }
            let _ = strip_png_metadata(&data);
            let _ = gif_frame_count(&data);
            let _ = process_outgoing_image(&data);
            let _ = crate::content::Content::decode(&data);
            let s = String::from_utf8_lossy(&data);
            let _ = is_single_emoji(&s);
            assert!(!clean_display_text(&s, 50).chars().any(is_invisible_format_char));
        }
    }
}
