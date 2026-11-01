#!/bin/bash
# Builds Relais.app with the relais command inside it, signed. Installs and
# starts nothing: the bundle is left in target/release/bundle/macos.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

cd "$root"
cargo build --release --locked -p relais-cli
cd "$root/app"
pnpm tauri build --bundles app

bundle="$root/target/release/bundle/macos/Relais.app"
# The shell function and the panel call this copy of the command.
cp "$root/target/release/relais" "$bundle/Contents/MacOS/relais"
"$root/app/widget/build.sh" "$bundle" "$(uname -m)"
extension="$bundle/Contents/PlugIns/RelaisWidget.appex"

# A certificate keeps the same signature from one build to the next, so macOS
# keeps the app's permissions and its login item; without one, ad hoc.
identity="${RELAIS_CODESIGN_IDENTITY:-}"
if [ -z "$identity" ] && [ -f "$HOME/.relais/build-signing-identity" ]; then
  identity="$(cat "$HOME/.relais/build-signing-identity")"
fi
if [ -n "$identity" ] && [ "$identity" != "-" ]; then
  codesign --force --timestamp --options runtime --sign "$identity" \
    --identifier ai.neoservice.relais.cli "$bundle/Contents/MacOS/relais"
  codesign --force --timestamp --options runtime --sign "$identity" \
    --entitlements "$root/app/widget/RelaisWidget.entitlements" "$extension"
  codesign --force --timestamp --options runtime --sign "$identity" \
    --entitlements "$root/app/widget/Relais.entitlements" "$bundle"
else
  codesign --force --sign - --identifier ai.neoservice.relais.cli "$bundle/Contents/MacOS/relais"
  codesign --force --sign - --entitlements "$root/app/widget/RelaisWidget.entitlements" "$extension"
  codesign --force --sign - --entitlements "$root/app/widget/Relais.entitlements" "$bundle"
fi
codesign --verify --deep --strict "$bundle"
echo "$bundle"
