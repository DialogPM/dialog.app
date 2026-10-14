# Supply chain and reproducible builds

What is in place, and what is still needed so that anyone can rebuild a released Dialog binary
from the tagged source and get the same bytes (Signal's `reproducible-builds/` does this for
Android). Gap analysis item 6.6 (`docs/SIGNAL-GAP-ANALYSIS.md`).

## In place

| What | Where |
|---|---|
| Rust toolchain pinned for CI: **1.94.1** (`rustc 1.94.1 (e408947bf 2026-03-25)`), installed with `rustup` (no toolchain action) | `.github/workflows/ci.yml` (`RUST_TOOLCHAIN`), `desktop.yml` |
| Every cargo command in CI uses `--locked` (`cargo fetch --locked` before `tauri build`) | both workflows |
| Third-party GitHub actions pinned to commit SHAs (taken from Signal's own workflows, which pin the same actions); the comment names the release | both workflows |
| `cargo-deny`: RustSec advisories, yanked crates, licenses compatible with AGPL-3.0-only, only crates.io + libsignal git sources | `deny.toml`, CI job `supply-chain` |
| `cargo audit` on both lock files (workspace and `server-rs/Cargo.lock`) | CI job `supply-chain` |
| JS dependencies installed with `pnpm install --frozen-lockfile` | both workflows |

### Still to pin

- `actions/setup-go` (ci.yml) and `arduino/setup-protoc` (desktop.yml) are still referenced by tag
  (marked `TODO(supply-chain)`): look up the commit of the release you trust
  (`git ls-remote https://github.com/actions/setup-go refs/tags/v5.x.y`) and replace the tag with
  `@<sha> # v5.x.y`. Dependabot (`package-ecosystem: github-actions`) can keep SHA pins current.
- `cargo install cargo-deny/cargo-audit` uses a semver range; pin exact versions once chosen.

### `rust-toolchain.toml` (to add at the repository root)

```toml
[toolchain]
channel = "1.94.1"
profile = "minimal"
components = ["rustfmt", "clippy"]
```

It is not committed at the root yet on purpose: with it, `rustup` installs the 1.94.1
toolchain on the first `cargo` command in the repository (a download on every developer
machine that only has `stable`). Run `rustup toolchain install 1.94.1 --profile minimal -c rustfmt -c clippy`
first, then add the file, and bump `channel`, `RUST_TOOLCHAIN` (ci.yml) and `desktop.yml`
together when upgrading.

## Needed for byte-for-byte reproducible releases

1. **Build environment in a container** with pinned digests: `rust:1.94.1-slim-bookworm@sha256:…`
   plus pinned apt packages (protoc, perl/make for the vendored OpenSSL of SQLCipher, Node 22 +
   pnpm 10 for the frontend). Server: `server-rs/Dockerfile` already builds in a container; pin
   its base images by digest.
2. **Deterministic Rust output**:
   - `--remap-path-prefix=$HOME=/home/build --remap-path-prefix=$PWD=/src` (no local paths in
     panics/debug info) and a fixed `CARGO_HOME` / checkout path;
   - `SOURCE_DATE_EPOCH` set from the release commit (`git log -1 --format=%ct`);
   - `codegen-units = 1` (already set in `[profile.release]`) and `lto = "thin"` are deterministic;
     keep `strip = true`;
   - vendored C code (SQLCipher, OpenSSL, libsignal's BoringSSL) must be built with the same
     compiler version: it comes from the container image.
3. **Frontend**: `pnpm install --frozen-lockfile`, `vite build` is deterministic given the same
   Node/pnpm versions: pin them in the image and add `"packageManager": "pnpm@10.x.y"` to
   `app/package.json` (not set yet).
4. **Installers**: macOS `.app`/`.dmg` and Windows MSI/NSIS embed timestamps and signatures.
   Compare the unsigned payload (the `.app` bundle contents / the executable) rather than the
   signed installer, as Signal does for the APK (they strip the signature block before `diff`).
5. **Verification script** (`reproducible-builds/verify.sh`, to write): checks out the tag, builds
   in the container, unpacks the published artifact, strips signatures and compares hashes.
   Publish the expected hashes with each release.
6. **Android/iOS** (mobile/): Android needs the release signing key first (gap 6.1); then the
   same approach as Signal-Android (`apkdiff`). iOS App Store builds cannot be reproduced
   byte-for-byte (Apple re-signs and encrypts them).

## Routine

- CI fails on a new RustSec advisory: upgrade the crate (`cargo update -p <crate>`), or, if it
  truly does not apply, add it to `deny.toml` `[advisories] ignore` with a comment.
- Adding a git dependency requires adding its URL to `allow-git` in `deny.toml` (review it).
- Desktop auto-update signing keys: `docs/DESKTOP-UPDATER.md`.
