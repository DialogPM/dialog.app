//! Names, file names and reactions from other people.
#![no_main]

use chat_core::sanitize::{clean_display_text, is_invisible_format_char, is_single_emoji, MAX_REACTION_BYTES};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|s: &str| {
    let clean = clean_display_text(s, 100);
    assert!(clean.chars().count() <= 100);
    assert!(!clean.chars().any(|c| c.is_control() || is_invisible_format_char(c)));
    if is_single_emoji(s) {
        assert!(s.len() <= MAX_REACTION_BYTES);
    }
});
