//! Metadata removal from outgoing images (also run on files a user picks, which may come
//! from anywhere): never panic; stripped PNG/GIF stay well-formed.
#![no_main]

use chat_core::sanitize::{gif_frame_count, process_outgoing_image, strip_gif_metadata, strip_png_metadata};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(png) = strip_png_metadata(data) {
        assert_eq!(strip_png_metadata(&png).as_deref(), Some(&png[..]), "stripping is idempotent");
    }
    if let Some(gif) = strip_gif_metadata(data) {
        assert_eq!(gif_frame_count(&gif), gif_frame_count(data));
    }
    if data.len() <= 1 << 20 {
        let _ = process_outgoing_image(data);
    }
});
