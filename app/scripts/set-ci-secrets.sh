#!/bin/bash
# Gives the Release workflow what it signs and notarizes with. Run once, from a
# terminal on a Mac holding the Developer ID certificate: the keychain asks you
# before the key leaves it. Nothing is printed, and no copy stays on disk.
#
# Usage: app/scripts/set-ci-secrets.sh /path/to/AuthKey_XXXX.p8 KEY_ID ISSUER_ID
set -euo pipefail

key="$1"
key_id="$2"
issuer="$3"
repo="bvisible/relais"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

password="$(openssl rand -base64 24)"
security export -t identities -f pkcs12 -P "$password" -o "$work/cert.p12"
base64 <"$work/cert.p12" | gh secret set MAC_CERT_P12 -R "$repo"
printf '%s' "$password" | gh secret set MAC_CERT_PASSWORD -R "$repo"
base64 <"$key" | gh secret set APPLE_API_KEY -R "$repo"
printf '%s' "$key_id" | gh secret set APPLE_API_KEY_ID -R "$repo"
printf '%s' "$issuer" | gh secret set APPLE_API_ISSUER -R "$repo"
gh secret set RELAIS_UPDATER_KEY -R "$repo" <"$HOME/.config/relais-updater.key"
echo "Done: the Release workflow can sign and notarize."
