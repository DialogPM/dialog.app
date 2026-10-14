# chat-core fuzz targets

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets for the code that parses
content from other people before anything else sees it (`src/content.rs`, `src/sanitize.rs`).
This directory is its own Cargo workspace: it is not a member of the main workspace and is
never built by `cargo build` / `cargo test` at the repository root.

| target           | what it feeds                                                           |
|------------------|-------------------------------------------------------------------------|
| `content_decode` | `Content::decode` (decrypted message payloads: size cap, unpad, JSON)    |
| `image_inspect`  | `inspect_image`, `sniff_image_mime`, `is_safe_thumbnail`, GIF walker     |
| `image_process`  | `process_outgoing_image`, PNG / GIF metadata strippers                   |
| `display_text`   | `clean_display_text` (bidi / invisible chars), `is_single_emoji`         |

## Running

Needs a nightly toolchain and cargo-fuzz:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
cd crates/chat-core
cargo +nightly fuzz list
cargo +nightly fuzz run image_inspect -- -max_total_time=600 -max_len=262144
cargo +nightly fuzz run content_decode -- -max_total_time=600
```

Seed corpora are optional; real photos / GIFs / PNGs dropped into
`fuzz/corpus/image_inspect/` (and `image_process/`) make the image targets reach deeper much
faster. Crashes land in `fuzz/artifacts/<target>/`; reproduce one with
`cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>`.

The normal test suite runs the same functions on a few hundred mutated inputs
(`sanitize::tests::decoders_survive_random_input`), so regressions show up in
`cargo test -p chat-core --lib` too.

Note: the fuzz build uses its own `target/` directory inside `fuzz/`; delete it after a
campaign if disk space matters.
