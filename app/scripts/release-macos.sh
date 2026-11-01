#!/bin/bash
# Builds what a person downloads: Relais.app signed with the Developer ID,
# notarized and stapled, in a DMG that is notarized and stapled in its turn
# (the app's ticket says nothing about the disk image around it).
#
# Needs the certificate in the keychain, and to notarize either `asc` signed in
# with a team key (on a Mac), or NOTARY_KEY, NOTARY_KEY_ID and NOTARY_ISSUER (in
# CI). RELAIS_ARCH_LABEL names the download when the build is not for this Mac.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
identity="${RELAIS_CODESIGN_IDENTITY:-}"
if [ -z "$identity" ] && [ -f "$HOME/.relais/build-signing-identity" ]; then
  identity="$(cat "$HOME/.relais/build-signing-identity")"
fi
if ! grep -F "$identity" <<<"$(security find-identity -v -p codesigning)" | grep -q "Developer ID Application"; then
  echo "The release needs a Developer ID Application certificate." >&2
  exit 1
fi
export RELAIS_CODESIGN_IDENTITY="$identity" RELAIS_UNIVERSAL=1

# Panic messages carry source paths: no home folder in a public binary.
export RUSTFLAGS="--remap-path-prefix=$HOME=/build ${RUSTFLAGS:-}"

"$root/app/scripts/build-macos.sh"

version="$(node -p "require('$root/app/src-tauri/tauri.conf.json').version")"
bundle="$root/target/universal-apple-darwin/release/bundle/macos/Relais.app"
out="$root/target/release-macos"
dmg="$out/Relais-$version-universal.dmg"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
rm -rf "$out"
mkdir -p "$out"

# Waiting ends well even when Apple refuses: only the status counts.
notarize() {
  if [ -n "${NOTARY_KEY:-}" ]; then
    xcrun notarytool submit "$1" --key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" \
      --issuer "$NOTARY_ISSUER" --wait --timeout 30m --output-format json >"$work/notary.json"
  else
    asc notarization submit --file "$1" --wait >"$work/notary.json"
  fi
  if ! grep -q '"status" *: *"Accepted"' "$work/notary.json"; then
    cat "$work/notary.json" >&2
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
# macOS's tar slips `._` files in, hides them when listing, and the updater then
# fails on them: the only proof they are gone is a reader other than tar.
COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -czf "$archive" -C "$(dirname "$bundle")" Relais.app
python3 -c '
import sys, tarfile
names = tarfile.open(sys.argv[1]).getnames()
sys.exit("macOS metadata in the update archive" if any(n.split("/")[-1].startswith("._") for n in names) else 0)
' "$archive"
(cd "$root/app" && pnpm tauri signer sign -f "${RELAIS_UPDATER_KEY_FILE:-$HOME/.config/relais-updater.key}" -p "" "$archive")
VERSION="$version" SIGNATURE="$(cat "$archive.sig")" node -e '
const { VERSION: version, SIGNATURE: signature } = process.env;
const url = `https://github.com/bvisible/relais/releases/download/v${version}/Relais.app.tar.gz`;
const platforms = Object.fromEntries(["darwin-aarch64", "darwin-x86_64"].map((name) => [name, { signature, url }]));
const manifest = { version, pub_date: new Date().toISOString(), platforms };
process.stdout.write(JSON.stringify(manifest, null, 2) + "\n");
' >"$out/latest.json"
