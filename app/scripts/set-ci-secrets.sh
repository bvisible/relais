#!/bin/bash
# Gives the Release workflow what it signs and notarizes with. Run once, from a
# terminal on a Mac holding the Developer ID certificate and signed in to `asc`:
# the keychain asks you before the key leaves it. Nothing is printed, and no
# copy stays on disk. Without `asc`, set NOTARY_KEY, NOTARY_KEY_ID and
# NOTARY_ISSUER first.
set -euo pipefail

config="$HOME/.asc/config.json"
read_config() {
  python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$config" "$1"
}
key_id="${NOTARY_KEY_ID:-$(read_config key_id)}"
issuer="${NOTARY_ISSUER:-$(read_config issuer_id)}"
key="${NOTARY_KEY:-$HOME/.config/AuthKey_$key_id.p8}"
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
