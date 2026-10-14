#!/usr/bin/env bash
# IPA pentru Dialog (Flutter) — App Store / TestFlight.
#
#   BUILD_NUMBER=15 ./build-ipa.sh          # numărul de build crește strict la fiecare upload
#   SKIP_TESTS=1 / SKIP_PREFLIGHT=1         # doar conștient
set -e
cd "$(dirname "$0")"
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 CARGO_INCREMENTAL=0
export CHATAPP_DEFAULT_SERVER="${CHATAPP_DEFAULT_SERVER:-https://chat.dialog.pm}"
: "${BUILD_NUMBER:?setează BUILD_NUMBER (ultimul urcat: vezi .last-build-code-ios)}"

# --- Poarta de calitate ------------------------------------------------------
# Un build urcat cu testele roșii nu se mai retrage decât cu altă versiune și încă un review.
if [ "${SKIP_TESTS:-0}" != "1" ]; then
  echo "==> flutter analyze"; flutter analyze lib || { echo "!! analyze a picat — nu construiesc."; exit 1; }
  echo "==> flutter test";    flutter test        || { echo "!! teste picate — nu construiesc."; exit 1; }
else
  echo "!! SKIP_TESTS=1 — construiesc FĂRĂ teste."
fi

# --- Verificări pe artefact (tool/app-preflight.sh) --------------------------
tool/app-preflight.sh ios || exit 1

OUT_DIR="../dist/release-$(date +%Y-%m-%d)"
echo "==> flutter build ipa (release, build $BUILD_NUMBER)"
flutter build ipa --release --build-number "$BUILD_NUMBER" \
  --export-options-plist ios/ExportOptions-AppStore.plist \
  --dart-define=CHATAPP_DEFAULT_SERVER="$CHATAPP_DEFAULT_SERVER"

# IPA-ul se caută după mtime: folderul păstrează IPA-uri vechi.
SRC="$(ls -t build/ios/ipa/*.ipa 2>/dev/null | head -1 || true)"
[ -z "$SRC" ] && { echo "!! niciun .ipa în build/ios/ipa"; exit 1; }
mkdir -p "$OUT_DIR"
cp -f "$SRC" "$OUT_DIR/Dialog-ios.ipa"
echo "$BUILD_NUMBER" > .last-build-code-ios
echo "==> IPA gata: $OUT_DIR/Dialog-ios.ipa (build $BUILD_NUMBER)"
echo "    Upload: ./upload-ipa.sh   (sau trage fișierul în Transporter)"
