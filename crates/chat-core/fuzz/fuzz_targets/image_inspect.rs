//! Header-only checks run on every downloaded image and thumbnail before the apps decode
//! them: never panic, never accept an image above the limits.
#![no_main]

use chat_core::sanitize::{gif_frame_count, inspect_image, is_safe_thumbnail, sniff_image_mime, MAX_INCOMING_PIXELS, MAX_INCOMING_SIDE};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = sniff_image_mime(data);
    let _ = gif_frame_count(data);
    if let Some((_, w, h)) = inspect_image(data, MAX_INCOMING_SIDE, MAX_INCOMING_PIXELS) {
        assert!(w <= MAX_INCOMING_SIDE && h <= MAX_INCOMING_SIDE);
        assert!(w as u64 * h as u64 <= MAX_INCOMING_PIXELS);
    }
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = is_safe_thumbnail(s);
    }
});
