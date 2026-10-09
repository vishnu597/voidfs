#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
#
# Builds voidfs.app as it ships: archived, signed with Developer ID (the extension with the
# Developer ID provisioning profile that grants FSKit Module), notarized and stapled, then
# checks the signature, Gatekeeper's verdict and the ticket.
#
#     apps/macos/scripts/release.sh [notarytool keychain profile, default voidfs-notary]
#
# The keychain profile comes from `xcrun notarytool store-credentials`. The result is
# apps/macos/build/release/voidfs.app.
set -euo pipefail
profile="${1:-voidfs-notary}"
here="$(cd "$(dirname "$0")/.." && pwd)"
out="$here/build/release"
rm -rf "$out"
mkdir -p "$out"
xcodebuild -project "$here/voidfs.xcodeproj" -scheme voidfs -configuration Release \
  -archivePath "$out/voidfs.xcarchive" -allowProvisioningUpdates archive
xcodebuild -exportArchive -archivePath "$out/voidfs.xcarchive" -exportPath "$out" \
  -exportOptionsPlist "$here/scripts/ExportOptions-DeveloperID.plist"
ditto -c -k --keepParent "$out/voidfs.app" "$out/voidfs.zip"
xcrun notarytool submit "$out/voidfs.zip" --keychain-profile "$profile" --wait
xcrun stapler staple "$out/voidfs.app"
codesign --verify --deep --strict --verbose=2 "$out/voidfs.app"
spctl --assess --type execute --verbose=4 "$out/voidfs.app"
xcrun stapler validate "$out/voidfs.app"
