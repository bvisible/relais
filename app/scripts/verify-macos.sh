#!/bin/bash
# Checks a DMG the way a download meets it. A file made on this Mac has no
# quarantine flag, so Gatekeeper would wave it through: the checks run on a copy
# that carries one. Usage: app/scripts/verify-macos.sh Relais-x.y.z-arch.dmg
set -euo pipefail

dmg="$1"
team="BT249938WK"
work="$(mktemp -d)"
mount="$work/mount"
cleanup() {
  hdiutil detach -quiet "$mount" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

fail() {
  echo "✗ $1" >&2
  exit 1
}
pass() {
  echo "✓ $1"
}

copy="$work/$(basename "$dmg")"
cp "$dmg" "$copy"
xattr -w com.apple.quarantine "0083;00000000;Safari;" "$copy"

xcrun stapler validate "$copy" >/dev/null || fail "the DMG carries no ticket"
pass "the DMG carries its ticket"
spctl -a -t open --context context:primary-signature "$copy" 2>/dev/null || fail "Gatekeeper refuses the DMG"
pass "Gatekeeper accepts the DMG"

hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mount" "$copy"
app="$mount/Relais.app"
cli="$app/Contents/MacOS/relais"

[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" = "ai.neoservice.relais" ] || fail "wrong bundle id"
pass "the bundle id is right"
codesign --verify --deep --strict "$app" || fail "the signature does not hold"
pass "the signature holds end to end"
# Read whole before searching: `grep -q` leaves early, and with pipefail the
# command it cut short would fail the check, or pass it when it should fail.
grep -q "Authority=Developer ID Application: .*($team)" <<<"$(codesign -dvv "$app" 2>&1)" || fail "not signed with the Developer ID"
pass "signed with the Developer ID of team $team"
for binary in "$app/Contents/MacOS/relais-app" "$cli"; do
  details="$(codesign -dvv "$binary" 2>&1)"
  grep -q 'flags=.*runtime' <<<"$details" || fail "$(basename "$binary") lacks the hardened runtime"
  grep -q '^Timestamp=' <<<"$details" || fail "$(basename "$binary") lacks a secure timestamp"
done
pass "both programs have the hardened runtime and a timestamp"
widget="$app/Contents/PlugIns/RelaisWidget.appex"
[ -d "$widget" ] || fail "the widget is missing"
for binary in "$app/Contents/MacOS/relais-app" "$cli" "$widget/Contents/MacOS/RelaisWidget"; do
  for arch in arm64 x86_64; do
    grep -qw "$arch" <<<"$(lipo -archs "$binary")" || fail "$(basename "$binary") lacks $arch"
  done
done
pass "the app, the command and the widget run on Apple Silicon and Intel"
xcrun stapler validate "$app" >/dev/null || fail "the app carries no ticket"
pass "the app carries its ticket"
spctl -a -t exec "$app" 2>/dev/null || fail "Gatekeeper refuses the app"
pass "Gatekeeper accepts the app"
"$cli" --version >/dev/null || fail "the relais command does not start"
pass "the relais command starts"
strings -a "$app/Contents/MacOS/relais-app" "$cli" "$widget/Contents/MacOS/RelaisWidget" >"$work/strings"
if grep -q -E '/Users/[a-z]' "$work/strings"; then
  fail "a home folder is written in a binary"
fi
pass "no home folder in the binaries"
