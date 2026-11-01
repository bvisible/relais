#!/bin/bash
# Compiles the widget and puts it in the app, for each architecture named.
# Usage: app/widget/build.sh Relais.app arm64 [x86_64]
set -euo pipefail

bundle="$1"
shift
here="$(cd "$(dirname "$0")" && pwd)"
extension="$bundle/Contents/PlugIns/RelaisWidget.appex"
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$bundle/Contents/Info.plist")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

rm -rf "$extension"
mkdir -p "$extension/Contents/MacOS"
sed "s/VERSION/$version/" "$here/Info.plist" >"$extension/Contents/Info.plist"
slices=()
for arch in "$@"; do
  swiftc -O -parse-as-library -application-extension \
    -target "$arch-apple-macos14.0" -sdk "$(xcrun --show-sdk-path)" \
    -framework SwiftUI -framework WidgetKit \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$work/$arch" "$here/RelaisWidget.swift"
  slices+=("$work/$arch")
done
lipo -create "${slices[@]}" -output "$extension/Contents/MacOS/RelaisWidget"
