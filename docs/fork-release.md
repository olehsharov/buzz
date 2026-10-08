# Fork desktop releases and auto-update

This runbook covers desktop releases for the `olehsharov/buzz` fork. They are
built by [`.github/workflows/fork-release.yml`](../.github/workflows/fork-release.yml)
and delivered to installed apps by the Tauri updater.

Upstream's `release.yml` cannot run on the fork. It depends on Block's OIDC
codesign action, four platforms, a CHANGELOG gate, and a rolling promote tag.
The fork workflow builds only:

| Platform | Artifacts | Auto-updates |
|---|---|---|
| macOS Apple Silicon (`darwin-aarch64`) | `Buzz_<ver>_aarch64.dmg`, `Buzz_<ver>_aarch64.app.tar.gz` + `.sig` | yes |
| Linux x86_64 (`linux-x86_64`) | `Buzz_<ver>_amd64.AppImage` + `.sig`, `Buzz_<ver>_amd64.deb` | AppImage only |

Every release also carries `latest.json`. Installed fork builds poll this URL:

```
https://github.com/olehsharov/buzz/releases/latest/download/latest.json
```

Each tag is published as the repo's **Latest** release, so the URL always
serves the newest fork build. There is no separate promote step. Builds have
this URL compiled in, so treat it as permanent: changing it strands every
existing install.

## How it fits together

- **Updater key.** A Tauri minisign key pair. The public half is committed as
  `desktop/fork-updater.pub` and compiled into the app. The private half is a
  repo secret that signs the `.app.tar.gz` and the `.AppImage`. An app only
  installs updates signed by the key it was built with. Upstream's key and the
  fork's key are not interchangeable.
- **macOS code signing.** The workflow imports the existing self-signed
  **"Buzz Local Code Signing"** certificate (SHA-1
  `8C583BAD48232DC40EBCF8AED2217E6EF812D8E7`) into a temporary keychain, and
  Tauri signs the app with hardened runtime and `Entitlements.plist`. This
  keeps the designated requirement unchanged:
  `identifier "xyz.block.buzz.app" and certificate leaf = H"8c583bad…"`. As a
  result, microphone and camera (TCC) grants and the login-keychain access to
  the stored nsec survive every update. CI fails the build if the requirement
  ever changes.
- **No notarization.** A downloaded DMG needs "Open Anyway" once. In-app
  updates are not quarantined, because the updater writes the bundle itself,
  so they need no extra step.

## One-time setup (repo owner)

### 1. Enable Actions on the fork

Open <https://github.com/olehsharov/buzz/actions> and click **"I understand my
workflows, go ahead and enable them"**.

This also enables upstream workflows such as CI on pushes to `main`/`release`
and on pull requests. Those workflows are guarded or harmless, but they use
runner minutes. The job-level `permissions: contents: write` in the publish
job is enough to create releases. If `gh release create` returns 403, set
**Settings → Actions → General → Workflow permissions → Read and write**.

### 2. Generate the updater key

Do this once. **Back up the private key and its password**, for example in
1Password. If the key is lost, installed apps can never update again without
a manual reinstall. Never commit the private key.

```bash
cd /Users/olh/Documents/dev/buzz && . ./bin/activate-hermit && cd desktop
pnpm tauri signer generate -w ~/.tauri/buzz-fork.key     # choose a password
cp ~/.tauri/buzz-fork.key.pub fork-updater.pub            # desktop/fork-updater.pub — commit this
gh secret set TAURI_SIGNING_PRIVATE_KEY --repo olehsharov/buzz < ~/.tauri/buzz-fork.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo olehsharov/buzz --body '<the password>'
```

Commit `desktop/fork-updater.pub`. If the file is missing from the tagged
commit, the workflow stops in its first job with an explicit error.

### 3. Export the macOS signing certificate

The p12 must hold **this exact certificate**. A different one changes the
designated requirement, and CI rejects it.

1. Keychain Access → **login** → **My Certificates** → **Buzz Local Code
   Signing** → right-click → **Export…** → save as `.p12` with a password.
2. Store it as secrets, then delete the file:

```bash
base64 -i ~/Desktop/buzz-local.p12 | gh secret set MACOS_SIGNING_P12_BASE64 --repo olehsharov/buzz
gh secret set MACOS_SIGNING_P12_PASSWORD --repo olehsharov/buzz --body '<p12 password>'
rm ~/Desktop/buzz-local.p12
```

If **Export…** is greyed out, the private key was created as non-extractable.
A new certificate would then be needed: update `EXPECTED_LEAF_SHA1` in the
workflow. Every install re-prompts once for mic/camera and keychain access.

### Secrets summary

| Secret | Value |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | contents of `~/.tauri/buzz-fork.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its password |
| `MACOS_SIGNING_P12_BASE64` | base64 of the exported `.p12` |
| `MACOS_SIGNING_P12_PASSWORD` | the `.p12` password |

The updater **public** key is not a secret. It lives in
`desktop/fork-updater.pub`. No Apple ID or notarization secrets are used.

## Version scheme

```
<branch version with patch + 1>-fork.<N>
```

- The branch's `desktop/src-tauri/tauri.conf.json` says `0.5.26`, so today's
  releases are **`0.5.27-fork.1`**, `0.5.27-fork.2`, and so on. `N` compares
  numerically, so `fork.10` > `fork.2`.
- After rebasing onto an upstream that ships `0.5.27`, move to
  `0.5.28-fork.1`.
- Why the patch bump: semver ranks a prerelease below its release. So
  `0.5.26-fork.N` < `0.5.26`, and an installed `0.5.26` would never see it as
  an update. Build metadata (`+fork.1`) is ignored by the comparison
  entirely.
- Releases are never marked prerelease, because `/releases/latest` skips
  prereleases.
- The tag must match `desktop-v<major>.<minor>.<patch>-fork.<N>`. The setup
  job rejects anything else. The workflow does not compare the tag with
  installed versions, so pick a version greater than every build already in
  use.

## Cutting a release

The workflow file and `desktop/fork-updater.pub` are read from the **tagged
commit**, so both must be committed on the branch you tag.

```bash
git push fork feat/admin-agent-integration
git tag desktop-v0.5.27-fork.1            # on the commit to release
git push fork desktop-v0.5.27-fork.1
```

Watch <https://github.com/olehsharov/buzz/actions>. Expect about 45–60 min for
the first run, which builds the metal llama cache, and about 30–40 min after.
The same tag also matches upstream `release.yml`. That run shows as skipped,
because its jobs are guarded to `block/buzz`.

The publish job creates a **draft**, uploads every asset including
`latest.json`, and only then publishes it as Latest. As a result,
`latest.json` never points at a half-uploaded release. Re-running a failed
run is safe. An existing draft for the same commit is reused, and an
already-published release is left alone.

## Bootstrap install (once per machine)

An app built without the updater (for example the current
`/Applications/Buzz.app` 0.5.26) cannot update itself. Install a fork build
manually once. Every later release then arrives in-app.

**Option A: local build, no Gatekeeper prompt.** Build a version just below
the first CI release, so that release becomes the first real auto-update.

```bash
cd /Users/olh/Documents/dev/buzz && . ./bin/activate-hermit
VERSION=0.5.27-fork.0
(cd desktop && node scripts/set-version-from-tag.mjs "$VERSION" && cd src-tauri && cargo update --workspace)
export BUZZ_UPDATER_PUBLIC_KEY="$(tr -d '\n' < desktop/fork-updater.pub)"
export BUZZ_UPDATER_ENDPOINT=https://github.com/olehsharov/buzz/releases/latest/download/latest.json
export VITE_BUZZ_RELEASES_URL=https://github.com/olehsharov/buzz/releases/latest
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/buzz-fork.key)" TAURI_SIGNING_PRIVATE_KEY_PASSWORD='<pw>'
export APPLE_SIGNING_IDENTITY="Buzz Local Code Signing"
(cd desktop && node scripts/build-release-config.mjs)
cargo build --release -p buzz-acp -p buzz-agent -p buzz-backend-kubernetes -p buzz-dev-mcp -p git-credential-nostr -p buzz-cli && ./scripts/bundle-sidecars.sh
(cd desktop && pnpm tauri build --features mesh-llm --config src-tauri/tauri.release.conf.json)
# Quit Buzz, then replace the app:
rm -rf /Applications/Buzz.app && ditto desktop/src-tauri/target/release/bundle/macos/Buzz.app /Applications/Buzz.app
# Restore the version-patched files:
git checkout -- desktop/package.json desktop/src-tauri/tauri.conf.json desktop/src-tauri/Cargo.toml desktop/src-tauri/Cargo.lock
```

Notes on Option A:

- Add your usual local build environment, for example
  `LIBOPUS_LIB_DIR=/opt/homebrew LIBOPUS_STATIC=1 LIBOPUS_NO_PKG=1`.
- The private key is needed because `createUpdaterArtifacts` makes tauri-cli
  sign the update archive.

**Option B: CI DMG.** Download the DMG from the release and drag Buzz to
Applications. The download is quarantined, so allow it once: System Settings →
Privacy & Security → **Open Anyway**, or run
`xattr -dr com.apple.quarantine /Applications/Buzz.app`. The next fork release
then validates the auto-update.

Both fork and official Buzz use the bundle id `xyz.block.buzz.app`. Installing
an official build over a fork build switches the update channel back to
upstream, and the reverse is also true. The signing identity changes at the
same time, so expect one mic/camera and keychain re-prompt.

## Linux

- **Use the AppImage.** Only the AppImage auto-updates. A `.deb` install shows
  "manual download required" with a link to the fork's releases page.
- Run `chmod +x Buzz_<ver>_amd64.AppImage` and keep it at a stable, writable
  path. The updater replaces the file in place.
- The AppImage needs FUSE 2. On Ubuntu 24.04 run
  `sudo apt install libfuse2t64`; on older releases install `libfuse2`.
  Without FUSE, run it with `--appimage-extract-and-run`.
- The binaries are built in an `ubuntu:24.04` container, so they need
  **glibc ≥ 2.39**: Ubuntu 24.04+, Debian 13, or Fedora 40+. Ubuntu 22.04
  (glibc 2.35) is not supported unless the workflow's container is changed to
  `ubuntu:22.04`.

## Verification checklist

1. **CI.** Each of these must hold:
   - The "Import signing certificate" step passes its SHA-1 check.
   - "Verify code signature" prints a designated requirement containing
     `certificate leaf = H"8c583bad48232dc40ebcf8aed2217e6ef812d8e7"`.
   - The entitlements check passes.
   - The publish job prints a `latest.json` with `darwin-aarch64` and
     `linux-x86_64`.
2. **Manifest.** Run these:

   ```bash
   curl -sL https://github.com/olehsharov/buzz/releases/latest/download/latest.json | jq .version   # = tag version
   curl -sL https://github.com/olehsharov/buzz/releases/latest/download/latest.json \
     | jq -r '.platforms[].url' | xargs -n1 curl -sIL -o /dev/null -w '%{http_code} %{url_effective}\n'   # all 200
   ```

   The release must be marked **Latest** and must not be a prerelease.
3. **macOS update.** Start from the bootstrap build.
   1. Settings → Check for updates shows "Update available — v<new>".
   2. Download it, then click **Update now**. The app relaunches.
   3. Check the result:
      - `defaults read /Applications/Buzz.app/Contents/Info.plist CFBundleShortVersionString`
        prints the new version.
      - `xattr -l /Applications/Buzz.app` shows no `com.apple.quarantine`.
      - `codesign -d -r- /Applications/Buzz.app` shows the same leaf hash.
      - The microphone permission is still granted.
      - The app signs in without re-pairing.
4. **Linux update.** Run the fork AppImage, publish the next fork release, and
   update in-app. The AppImage file is replaced in place and starts at the new
   version.

## Troubleshooting

### "Import signing certificate" fails or times out

codesign refuses an untrusted self-signed identity (`Buzz Local Code Signing:
no identity found`). So the step must mark the certificate as trusted for code
signing in the runner's System keychain. It does this through
[`scripts/fork-release-trust-signing-cert.sh`](../scripts/fork-release-trust-signing-cert.sh).

Apple blocks non-interactive admin trust changes. Hosted runner images differ
in how they handle this, and an image update can break it without notice:

- `security authorizationdb write com.apple.trust-settings.admin allow` was
  reported failing on `macos-15` with `NO (-60005)` while working on
  `macos-14`
  ([actions/runner-images#11893](https://github.com/actions/runner-images/issues/11893)).
  The script only warns here and still tries the trust change.
- `security add-trusted-cert` then fails with
  `SecTrustSettingsSetTrustSettings: The authorization was denied since no user
  interaction was possible`.
- Trust-settings commands were also reported hanging on macOS 14.7.5+ images
  ([actions/runner-images#12116](https://github.com/actions/runner-images/issues/12116)).

Every trust call is capped at 40 seconds and the step at 5 minutes. A hang
therefore fails within minutes instead of using the job's 120-minute timeout.
On any failure, the step logs `sw_vers`, `security dump-trust-settings -d`, and
`security find-identity -p codesigning` (which includes untrusted identities)
before it exits.

`runs-on: macos-latest` moves to new macOS versions over time. If the step
fails, the log shows which image ran. There are two ways out.

1. **Pin an older image.** Change the `macos-arm64` job to an image where the
   trust change still works, such as `runs-on: macos-14` if GitHub still
   offers it. This is the smallest change, but it only lasts until that image
   is retired or updated.
2. **Sign on your Mac.** The certificate is already trusted on your Mac, and
   this option also removes the need to export the p12. It requires a
   workflow change, done the way upstream `release.yml` does it:
   1. In CI, build with `pnpm tauri build --no-sign` and drop the
      p12-import step.
   2. On your Mac, download the built `Buzz.app` and sign it with hardened
      runtime and the entitlements:
      `codesign --force --deep --options runtime --entitlements desktop/src-tauri/Entitlements.plist -s "Buzz Local Code Signing" Buzz.app`.
      Then check the designated requirement with `codesign -d -r- Buzz.app`
      and run `desktop/scripts/verify-macos-entitlements.sh Buzz.app`.
   3. Pack and minisign the update archive:
      `tar -czf Buzz.app.tar.gz Buzz.app`, then
      `(cd desktop && pnpm tauri signer sign /abs/path/Buzz.app.tar.gz)` with
      `TAURI_SIGNING_PRIVATE_KEY` and its password set.
   4. Upload the archive and its `.sig` with `gh release upload`. The
      `darwin-aarch64` signature in `latest.json` must be regenerated from
      the new `.sig`. The DMG must also be rebuilt from the signed app,
      because the CI DMG contains the unsigned one.
