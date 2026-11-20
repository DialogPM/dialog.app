#!/usr/bin/env bash
# IPA pentru Dialog (Flutter) — App Store / TestFlight.
#
#   ./build-ipa.sh                          # numărul de build: ultimul din .last-build-code-ios + 1
#   BUILD_NUMBER=25 ./build-ipa.sh          # sau un număr anume (mai mare decât ultimul urcat)
#   BUILD_NAME=1.0.0 ./build-ipa.sh         # altă versiune decât cea din pubspec.yaml (ex. cea aflată în review)
#   SKIP_TESTS=1 / SKIP_PREFLIGHT=1         # doar conștient
set -e
cd "$(dirname "$0")"
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 CARGO_INCREMENTAL=0
export CHATAPP_DEFAULT_SERVER="${CHATAPP_DEFAULT_SERVER:-https://chat.dialog.pm}"
LAST="$(cat .last-build-code-ios 2>/dev/null || echo 0)"
if [ -z "${BUILD_NUMBER:-}" ]; then
  BUILD_NUMBER=$((LAST + 1))
elif [ "$BUILD_NUMBER" -le "$LAST" ]; then
  echo "!! BUILD_NUMBER=$BUILD_NUMBER a fost deja urcat (ultimul: $LAST) — Apple îl refuză."; exit 1
fi
# Și verificările de dinaintea build-ului (tool/app-preflight.sh) îl citesc de aici.
export BUILD_NUMBER
echo "==> build $BUILD_NUMBER (ultimul urcat: $LAST)"

# --- Poarta de calitate ------------------------------------------------------
# Un build urcat cu testele roșii nu se mai retrage decât cu altă versiune și încă un review.
if [ "${SKIP_TESTS:-0}" != "1" ]; then
  # Nucleul (chat-core, serverul, e2e pe server real) — aplicația mobilă rulează același cod.
  ../scripts/check.sh --no-flutter || { echo "!! testele nucleului au picat — nu construiesc."; exit 1; }
  echo "==> flutter analyze"; flutter analyze lib || { echo "!! analyze a picat — nu construiesc."; exit 1; }
  echo "==> flutter test";    flutter test        || { echo "!! teste picate — nu construiesc."; exit 1; }
else
  echo "!! SKIP_TESTS=1 — construiesc FĂRĂ teste."
fi

# --- Verificări pe artefact (tool/app-preflight.sh) --------------------------
tool/app-preflight.sh ios || exit 1

OUT_DIR="../dist/release-$(date +%Y-%m-%d)"
echo "==> flutter build ipa (release, build $BUILD_NUMBER)"
flutter build ipa --release --build-number "$BUILD_NUMBER" ${BUILD_NAME:+--build-name "$BUILD_NAME"} \
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
