#!/usr/bin/env bash
# Urcă ultimul archive (build/ios/archive/Runner.xcarchive) în App Store Connect, cum s-a
# făcut la build-urile 12–14: export cu destinația „upload” (ios/ExportOptions-Upload.plist,
# semnare automată cu contul din Xcode). Alternativ: Transporter cu dist/.../Dialog-ios.ipa.
set -e
cd "$(dirname "$0")"
LOG="build/ios/upload.log"
xcodebuild -exportArchive -archivePath build/ios/archive/Runner.xcarchive \
  -exportOptionsPlist ios/ExportOptions-Upload.plist -exportPath build/ios/upload \
  -allowProvisioningUpdates > "$LOG" 2>&1 || { echo "!! upload picat, vezi $LOG"; grep -E "error" "$LOG" | head -5; exit 1; }
grep -E "Upload succeeded|EXPORT SUCCEEDED" "$LOG" | tail -2
rm -rf build/ios/upload
