#!/usr/bin/env bash
# Verify the security and compatibility contract of a release artifact. This
# intentionally rejects local/ad-hoc builds: it is for the protected tag job.
set -euo pipefail

app_path="${1:-}"
dmg_path="${2:-}"
expected_version="${3:-}"

fail() {
  echo "macOS release verification failed: $*" >&2
  exit 1
}

[[ -d "$app_path" ]] || fail "application bundle not found: ${app_path:-<empty>}"
[[ -f "$dmg_path" ]] || fail "DMG not found: ${dmg_path:-<empty>}"

plist="$app_path/Contents/Info.plist"
[[ -f "$plist" ]] || fail "Info.plist is missing"

bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist")"
[[ "$bundle_id" == "io.github.hunter255.utter" ]] || fail "unexpected bundle id: $bundle_id"

minimum_macos="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$plist")"
[[ "$minimum_macos" == "13.0" ]] || fail "unexpected minimum macOS: $minimum_macos"

microphone_reason="$(/usr/libexec/PlistBuddy -c 'Print :NSMicrophoneUsageDescription' "$plist")"
[[ -n "$microphone_reason" ]] || fail "microphone purpose string is empty"

executable_name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$plist")"
executable="$app_path/Contents/MacOS/$executable_name"
[[ -f "$executable" ]] || fail "main executable is missing"
architectures="$(lipo -archs "$executable")"
[[ "$architectures" == "arm64" ]] || fail "expected ARM64-only binary, got: $architectures"

if [[ -n "$expected_version" ]]; then
  actual_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")"
  [[ "$actual_version" == "$expected_version" ]] || fail "version $actual_version does not match tag $expected_version"
fi

signature="$(codesign -dv --verbose=4 "$app_path" 2>&1)"
[[ "$signature" == *"Authority=Developer ID Application:"* ]] || fail "Developer ID Application signature is missing"
[[ "$signature" != *"TeamIdentifier=not set"* ]] || fail "Apple team identifier is missing"

codesign --verify --deep --strict --verbose=2 "$app_path"
codesign --verify --deep --strict --verbose=2 "$dmg_path"
xcrun stapler validate "$app_path"
xcrun stapler validate "$dmg_path"
spctl --assess --type execute --verbose=2 "$app_path"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg_path"

echo "Verified signed, notarized ARM64 release: $dmg_path"
