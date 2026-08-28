# Releasing Utter

Tagged releases are assembled as GitHub draft releases. Linux and macOS build
independently; a maintainer reviews the draft before making it public.

## macOS prerequisites

Direct distribution requires a paid Apple Developer membership and a
`Developer ID Application` certificate. Create a GitHub environment named
`release`, restrict it to protected version tags and require a reviewer, then
add these environment secrets in `hunter255/utter`:

- `APPLE_CERTIFICATE` — base64-encoded exported `.p12`;
- `APPLE_CERTIFICATE_PASSWORD` — the `.p12` export password;
- `APPLE_ID` — the Apple account used for notarization;
- `APPLE_PASSWORD` — an app-specific password, not the account password;
- `APPLE_TEAM_ID` — the Developer Program team identifier.

The tag-only workflow is the only consumer of these values. Pull-request CI
does not request or receive them. Never commit a certificate, password, Apple
identifier, or updater private key.

## Release procedure

1. Make the workspace version, `tauri.conf.json` version and UI package version
   identical.
2. Merge only a commit that passed all PR checks and the macOS manual matrix.
3. Push a tag of exactly `v<version>`; a mismatch stops before packaging.
4. Wait for both release matrix jobs. The macOS job signs with the permanent
   `io.github.hunter255.utter` identity, asks Apple to notarize, staples the
   app and DMG tickets, runs `scripts/verify-macos-release.sh`, then replaces
   the initial unpublished draft asset with the verified disk image.
5. Download the draft DMG on another Mac through a browser, move Utter to
   `/Applications`, and verify Gatekeeper, dictation, injection, models and
   launch-at-login before publishing the draft.

If verification fails, leave the draft unpublished, fix the cause, delete the
failed draft/tag only after preserving its logs, bump the version, and produce
a new tag. Published artifacts are immutable; do not replace a DMG under an
existing version.

Auto-update artifacts use a separate Tauri signing key, independent from the
Apple Developer certificate. Generate it once on a trusted machine and store
both files outside the repository:

```sh
apps/desktop/ui/node_modules/.bin/tauri signer generate \
  --write-keys /secure/location/utter-updater.key
```

Keep multiple encrypted backups. Losing this private key means existing
installations cannot trust any future update. Add its contents as the protected
environment secret `TAURI_SIGNING_PRIVATE_KEY`, its password as
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, and the generated public key as the
non-secret environment variable `UTTER_UPDATER_PUBLIC_KEY`.

The macOS tag build is compiled with the release-only `updater` feature and
`tauri.updater.conf.json`. Tauri signs the update archive, and `tauri-action`
adds the archive, signature and `latest.json` to the same draft release. The
application accepts only manifests and artifacts verified by the embedded
public key; this signature check cannot be disabled.
