#!/bin/bash
# Imports the release signing certificate into a temporary keychain on a macOS runner and exports its name as
# APPLE_SIGNING_IDENTITY, so the app is signed with the same certificate in every release. macOS ties the keychain's
# "Always Allow" to the signing certificate; an ad-hoc signature changes with every build, so the choice never sticks.
#
# Needs MACOS_SIGNING_CERT_P12 (base64 of the .p12) and MACOS_SIGNING_CERT_PASSWORD. Does nothing when the
# certificate isn't set, and the build is then signed ad hoc as before. See docs/RELEASING.md.
set -euo pipefail

if [ -z "${MACOS_SIGNING_CERT_P12:-}" ]; then
  echo "No signing certificate is set; the app will be signed ad hoc."
  exit 0
fi

identity="N-voke Release Signing"
keychain="$RUNNER_TEMP/nvoke-signing.keychain-db"
keychain_password="$(uuidgen)"

echo "$MACOS_SIGNING_CERT_P12" | base64 --decode > "$RUNNER_TEMP/signing.p12"

security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$RUNNER_TEMP/signing.p12" -k "$keychain" -P "$MACOS_SIGNING_CERT_PASSWORD" -T /usr/bin/codesign
rm -f "$RUNNER_TEMP/signing.p12"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" > /dev/null

# codesign only uses a self-signed certificate once it is trusted for code signing.
security find-certificate -c "$identity" -p "$keychain" > "$RUNNER_TEMP/signing.pem"
sudo security add-trusted-cert -d -r trustRoot -p codeSign -k "$keychain" "$RUNNER_TEMP/signing.pem"
rm -f "$RUNNER_TEMP/signing.pem"

# shellcheck disable=SC2046
security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')

security find-identity -v -p codesigning "$keychain"
echo "APPLE_SIGNING_IDENTITY=$identity" >> "$GITHUB_ENV"
