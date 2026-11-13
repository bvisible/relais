#!/bin/bash
# Points the Homebrew cask at the release just published. Run after the DMG is
# on GitHub: Homebrew checks its download against the sum written here.
# Usage: app/scripts/update-cask.sh 0.3.0
set -euo pipefail

version="$1"
root="$(cd "$(dirname "$0")/../.." && pwd)"
sum="$(cut -d' ' -f1 "$root/target/release-macos/Relais-$version-universal.dmg.sha256")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

git clone -q https://github.com/bvisible/homebrew-tap.git "$work/tap"
cd "$work/tap"
sed -i '' -E "s/^  version \".*\"/  version \"$version\"/; s/^  sha256 \".*\"/  sha256 \"$sum\"/" Casks/relais.rb
git add Casks/relais.rb
git commit -q -m "Relais $version"
git push -q origin main
echo "Cask at $version"
