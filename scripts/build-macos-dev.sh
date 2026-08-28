#!/usr/bin/env bash

# Build a permission-stable local Utter.app.
#
# A raw Rust/Tauri `--no-sign` build is only linker-signed. Its designated
# requirement is the executable's changing CDHash, so macOS TCC treats the
# next rebuild as a different microphone/accessibility client. This script
# signs the completed bundle and gives local builds one stable requirement.
# It is strictly a development workflow, not a distribution signature.

set -euo pipefail

script_dir="$(cd -P -- "$(dirname -- "$0")" && pwd)"
repo_root="$(cd -P -- "$script_dir/.." && pwd)"
tauri_dir="$repo_root/apps/desktop/src-tauri"
tauri_cli="$repo_root/apps/desktop/ui/node_modules/.bin/tauri"
app_bundle="$repo_root/target/release/bundle/macos/Utter.app"
entitlements="$tauri_dir/Entitlements.plist"
features="${UTTER_MACOS_FEATURES:-sherpa,whisper-metal}"
identity="${UTTER_DEV_SIGNING_IDENTITY:--}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this build helper only runs on macOS" >&2
  exit 1
fi

if [[ ! -x "$tauri_cli" ]]; then
  echo "error: Tauri CLI is missing; run 'npm ci' in apps/desktop/ui first" >&2
  exit 1
fi

build_args=(build --bundles app --no-sign)
if [[ -n "$features" ]]; then
  build_args+=(--features "$features")
fi

(
  cd "$tauri_dir"
  "$tauri_cli" "${build_args[@]}" "$@"
)

if [[ ! -d "$app_bundle" ]]; then
  echo "error: Tauri did not create $app_bundle" >&2
  exit 1
fi

bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app_bundle/Contents/Info.plist")"
sign_args=(
  --force
  --sign "$identity"
  --identifier "$bundle_id"
  --entitlements "$entitlements"
  --options runtime
  --timestamp=none
)

if [[ "$identity" == "-" ]]; then
  # Default ad-hoc signatures synthesize `cdhash H"..."` as their designated
  # requirement, which changes on every build. The explicit requirement is
  # intentionally limited to local development: it gives TCC a stable key
  # without pretending to authenticate a distributable build.
  sign_args+=(--requirements "=designated => identifier \"$bundle_id\"")
fi

/usr/bin/codesign "${sign_args[@]}" "$app_bundle"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$app_bundle"

echo
echo "Built and signed: $app_bundle"
echo "Bundle identifier: $bundle_id"
if [[ "$identity" == "-" ]]; then
  echo "Signing: local ad-hoc identity with a stable development requirement"
  echo "Do not distribute this bundle. Set UTTER_DEV_SIGNING_IDENTITY to a Keychain code-signing identity for a stronger local signature."
else
  echo "Signing identity: $identity"
fi
/usr/bin/codesign --display --requirements - "$app_bundle" 2>&1
