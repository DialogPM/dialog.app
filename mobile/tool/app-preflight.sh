#!/usr/bin/env bash
# Verificări pe ARTEFACT, înainte de a construi Dialog pentru magazine (ios | android).
#
# `flutter analyze` și `flutter test` spun că logica e bună. Astea spun că lucrul pe care
# urmează să-l urci e bun: identificatorul de push se potrivește cu aplicația, serverul
# implicit e producția, numărul de build e nou, iconițele iOS n-au transparență, fiecare
# permisiune are textul ei de scop. Toate sunt strict de citire.
#
# Folosire (din scriptul de build): tool/app-preflight.sh ios      Ocolire: SKIP_PREFLIGHT=1
set -e
PLATFORMA="${1:?argument: ios | android}"
[ "${SKIP_PREFLIGHT:-0}" = "1" ] && { echo "!! SKIP_PREFLIGHT=1 — sar peste verificările de artefact."; exit 0; }
cd "$(dirname "$0")/.."
PROD="https://chat.dialog.pm"
PROBLEME=0
avert() { echo "   ⚠️  $*"; }
pica()  { echo "   ❌ $*"; PROBLEME=$((PROBLEME + 1)); }
bun()   { echo "   ✅ $*"; }

LINIE_VER="$(grep -E '^version:' pubspec.yaml | head -1 | awk '{print $2}')"
VERSIUNE="${LINIE_VER%%+*}"
COD="${BUILD_NUMBER:-${LINIE_VER##*+}}"
echo ""
echo "==> Verificări înainte de build  (dialog / $PLATFORMA, v$VERSIUNE build $COD)"

# ─── 1. Numărul de build e nou ───────────────────────────────────────────────
# App Store Connect și Play refuză un număr deja folosit, dar abia după upload. Ținem local
# ultimul construit (.last-build-code, negitat), ca refuzul să vină în secunda 1.
echo "-- numărul de build"
URMA=".last-build-code-$PLATFORMA"
if [ -f "$URMA" ]; then
  ULTIM="$(cat "$URMA")"
  if [ "$COD" -le "$ULTIM" ] 2>/dev/null; then
    pica "build $COD <= ultimul construit ($ULTIM). Magazinul îl refuză: crește BUILD_NUMBER."
  else
    bun "build $COD (anterior: $ULTIM)"
  fi
else
  bun "build $COD (prima urmă locală)"
fi

# ─── 2. Serverul implicit e producția ────────────────────────────────────────
# Serverul vine din --dart-define=CHATAPP_DEFAULT_SERVER; scripturile de build îl trimit.
echo "-- adresa serverului"
case "${CHATAPP_DEFAULT_SERVER:-}" in
  "$PROD") bun "CHATAPP_DEFAULT_SERVER e producția" ;;
  "") pica "CHATAPP_DEFAULT_SERVER nesetat: aplicația ar porni fără server implicit." ;;
  *) pica "CHATAPP_DEFAULT_SERVER='$CHATAPP_DEFAULT_SERVER' nu e producția ($PROD)." ;;
esac

# ─── 3. Identificatorul de push ──────────────────────────────────────────────
# Un fișier Firebase al altui pachet lasă push-ul mort în tăcere. Se citește structura.
if [ "$PLATFORMA" = "ios" ]; then
  echo "-- GoogleService-Info.plist ↔ bundle id"
  GS="ios/Runner/GoogleService-Info.plist"
  BID="$(grep -E 'PRODUCT_BUNDLE_IDENTIFIER = [^;]+;' ios/Runner.xcodeproj/project.pbxproj | grep -v RunnerTests | head -1 | sed 's/.*= \(.*\);/\1/')"
  if [ ! -f "$GS" ]; then
    pica "$GS lipsește: notificările push nu vor funcționa."
  else
    GSB="$(/usr/libexec/PlistBuddy -c 'Print :BUNDLE_ID' "$GS" 2>/dev/null || echo "")"
    [ "$GSB" = "$BID" ] && bun "$BID e declarat în GoogleService-Info.plist" || pica "GoogleService-Info.plist e pentru '$GSB', nu pentru '$BID'."
  fi
else
  echo "-- google-services.json ↔ applicationId"
  GS="android/app/google-services.json"
  APPID="$(grep -hE 'applicationId' android/app/build.gradle.kts 2>/dev/null | head -1 | sed 's/.*"\(.*\)".*/\1/')"
  PACHETE="$(python3 -c "
import json, sys
try: d = json.load(open('$GS'))
except Exception: sys.exit('EROARE')
print(' '.join(c['client_info']['android_client_info']['package_name'] for c in d.get('client', [])))" 2>/dev/null || echo EROARE)"
  if [ ! -f "$GS" ]; then pica "$GS lipsește."
  elif [ "$PACHETE" = "EROARE" ]; then pica "$GS nu se poate citi ca JSON."
  elif printf '%s' " $PACHETE " | grep -q " $APPID "; then bun "$APPID e declarat în google-services.json"
  else pica "applicationId ($APPID) nu e în $GS (conține: $PACHETE)."; fi
fi

# ─── 4. iOS: iconițe fără transparență, texte de scop, export compliance ─────
if [ "$PLATFORMA" = "ios" ]; then
  echo "-- iconițe iOS"
  ALFA=0
  for f in ios/Runner/Assets.xcassets/AppIcon.appiconset/*.png; do
    sips -g hasAlpha "$f" 2>/dev/null | grep -q "yes" && { ALFA=$((ALFA + 1)); avert "transparență: $f"; }
  done
  [ "$ALFA" -eq 0 ] && bun "nicio iconiță cu canal alfa" || pica "$ALFA iconițe cu transparență: App Store le respinge."
  echo "-- texte de scop pentru permisiuni"
  for k in NSCameraUsageDescription NSMicrophoneUsageDescription NSPhotoLibraryUsageDescription NSPhotoLibraryAddUsageDescription NSContactsUsageDescription NSLocationWhenInUseUsageDescription NSFaceIDUsageDescription; do
    /usr/libexec/PlistBuddy -c "Print :$k" ios/Runner/Info.plist >/dev/null 2>&1 && bun "$k" || pica "$k lipsește din Info.plist"
  done
  echo "-- export compliance"
  /usr/libexec/PlistBuddy -c "Print :ITSAppUsesNonExemptEncryption" ios/Runner/Info.plist >/dev/null 2>&1 && bun "ITSAppUsesNonExemptEncryption declarat" || pica "ITSAppUsesNonExemptEncryption lipsește: App Store Connect îl va cere la fiecare build."
  echo "-- manifest de confidențialitate"
  [ -f ios/Runner/PrivacyInfo.xcprivacy ] && bun "PrivacyInfo.xcprivacy" || pica "PrivacyInfo.xcprivacy lipsește"
fi

echo ""
if [ "$PROBLEME" -gt 0 ]; then
  echo "❌ $PROBLEME probleme — nu construiesc.  Conștient? SKIP_PREFLIGHT=1"
  exit 1
fi
echo "✅ Artefactul e în regulă."
echo ""
