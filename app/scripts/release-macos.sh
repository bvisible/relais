#!/bin/bash
# Builds what a person downloads: Relais.app signed with the Developer ID,
# notarized and stapled, in a DMG that is notarized and stapled in its turn
# (the app's ticket says nothing about the disk image around it).
#
# Needs the certificate in the keychain and `asc`, signed in with a team key:
# the notary credentials stay in its keychain item, this script never sees them.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
identity="${RELAIS_CODESIGN_IDENTITY:-}"
if [ -z "$identity" ] && [ -f "$HOME/.relais/build-signing-identity" ]; then
  identity="$(cat "$HOME/.relais/build-signing-identity")"
fi
security find-identity -v -p codesigning | grep -F "$identity" | grep -q "Developer ID Application" || {
  echo "The release needs a Developer ID Application certificate." >&2
  exit 1
}
export RELAIS_CODESIGN_IDENTITY="$identity"

# Panic messages carry source paths: no home folder in a public binary.
export RUSTFLAGS="--remap-path-prefix=$HOME=/build ${RUSTFLAGS:-}"

"$root/app/scripts/build-macos.sh"

version="$(node -p "require('$root/app/src-tauri/tauri.conf.json').version")"
bundle="$root/target/release/bundle/macos/Relais.app"
out="$root/target/release/bundle/release"
dmg="$out/Relais-$version-$(uname -m).dmg"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

# Waiting ends well even when Apple refuses: only the status counts.
notarize() {
  asc notarization submit --file "$1" --wait >"$work/notary.json"
  if ! grep -q '"status" *: *"Accepted"' "$work/notary.json"; then
    cat "$work/notary.json" >&2
    id="$(sed -nE 's/.*"id" *: *"([^"]+)".*/\1/p' "$work/notary.json" | head -1)"
    asc notarization log --id "$id" >&2 || true
    exit 1
  fi
  echo "Notarized: $(basename "$1")"
}

ditto -c -k --keepParent "$bundle" "$work/Relais.zip"
notarize "$work/Relais.zip"
xcrun stapler staple "$bundle"

mkdir "$work/image"
ditto "$bundle" "$work/image/Relais.app"
ln -s /Applications "$work/image/Applications"
hdiutil create -quiet -volname Relais -srcfolder "$work/image" -format UDZO -ov "$dmg"
codesign --force --timestamp --sign "$identity" "$dmg"
notarize "$dmg"
xcrun stapler staple "$dmg"

"$root/app/scripts/verify-macos.sh" "$dmg"
(cd "$out" && shasum -a 256 "$(basename "$dmg")" | tee "$(basename "$dmg").sha256")

# What the app downloads to update itself: the notarized app, signed with the
# updater's own key. Without that key, an app already installed refuses it.
archive="$out/Relais.app.tar.gz"
tar -czf "$archive" -C "$(dirname "$bundle")" Relais.app
(cd "$root/app" && pnpm tauri signer sign -f "${RELAIS_UPDATER_KEY:-$HOME/.config/relais-updater.key}" -p "" "$archive")
VERSION="$version" SIGNATURE="$(cat "$archive.sig")" node -e '
const { VERSION: version, SIGNATURE: signature } = process.env;
const url = `https://github.com/bvisible/relais/releases/download/v${version}/Relais.app.tar.gz`;
const platforms = Object.fromEntries(["darwin-aarch64", "darwin-x86_64"].map((name) => [name, { signature, url }]));
const manifest = { version, pub_date: new Date().toISOString(), platforms };
process.stdout.write(JSON.stringify(manifest, null, 2) + "\n");
' >"$out/latest.json"
