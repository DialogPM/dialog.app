//! Decrypted message payloads (JSON `Content`, after unpadding): never panic, and whatever
//! parses must serialize again.
#![no_main]

use chat_core::content::Content;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(c) = Content::decode(data) {
        let again = Content::decode(&c.encode()).expect("re-encoded content decodes");
        let _ = again;
    }
});
