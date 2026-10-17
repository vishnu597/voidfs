#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
#
# Xcode's "Bundle void" phase of the voidfs target: builds the `void` command line from this
# repository and puts it in the app at Contents/MacOS/void, signed as the app is, for the daemon
# that Start at login runs (step 5, item 7; LaunchAgents/dev.voidfs.app.daemon.plist). Release
# builds it with `cargo build --release`, Debug without. It builds for this Mac's architecture:
# macOS 27 runs on Apple silicon only.
set -euo pipefail
repo="$(cd "$SRCROOT/../.." && pwd)"
# Xcode's PATH has no cargo. /usr/bin comes before /usr/local/bin, whose tools can be Intel builds.
export PATH="$HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin"
# A target directory of its own, so that this build and a terminal's don't wait on each other.
export CARGO_TARGET_DIR="$SRCROOT/build/cargo-target"
if [ "$CONFIGURATION" = Release ]; then profile=release; flags=(--release); else profile=debug; flags=(); fi
cargo build --manifest-path "$repo/Cargo.toml" -p voidfs-cli --bin void --locked ${flags[@]+"${flags[@]}"}
dest="$TARGET_BUILD_DIR/$EXECUTABLE_FOLDER_PATH/void"
# A new file rather than one written over: macOS kills a process whose signed binary changes.
rm -f "$dest"
cp "$CARGO_TARGET_DIR/$profile/void" "$dest"
if [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ]; then
  # An archive is signed with a timestamp, as notarization asks; other builds without one.
  if [ "$ACTION" = install ]; then timestamp=--timestamp; else timestamp=--timestamp=none; fi
  codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" --identifier dev.voidfs.void --options runtime "$timestamp" "$dest"
fi
