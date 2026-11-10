# Releasing Irori

Two workflows ship Irori:

- **`release.yml`** builds every package and tests that it installs and starts. A
  pull request that touches packaging runs it without publishing. Pushing a `vX.Y.Z`
  tag runs it and leaves a **draft** GitHub release with every package, a
  `SHA256SUMS` file and build attestations.
- **`publish.yml`** runs when you publish that draft. It sends the release to each
  package channel that is switched on. Each channel is off until its repository
  variable is `true`, so forks and repositories without the secrets skip it.

`packaging/release_manifests.py` writes every channel's manifest from the release's
files (the AUR's PKGBUILD, the Homebrew cask, the Scoop manifest, the COPR spec, the
winget manifests and, given the tag's commit, the Flathub manifest), and
`packaging/test_release_manifests.py` tests them. Pull requests that touch packaging
also run, in `release.yml`: the COPR source package rebuilt and installed in Fedora
(`packaging/copr/build.sh`), the AUR PKGBUILD read by `makepkg --printsrcinfo`
(`packaging/arch/build.sh`), the Windows setup installed with winget's
`/CHANNEL=winget` switch, the Flatpak built from the manifest Flathub gets, and the
Windows zip Scoop unpacks.

Irori never updates itself: a package a channel installed is updated by that
channel's package manager (`irori-services::app_update` tells them apart; see
[PLATFORMS.md](PLATFORMS.md#new-versions-of-irori)).

## Cutting a release

1. Set `version` in the root `Cargo.toml` and the `<release>` in
   `packaging/linux/app.irori.Irori.metainfo.xml`. Merge that change.
2. Tag `main` and push the tag: `git tag v0.2.0 && git push origin v0.2.0`. The
   workflow fails if the tag and the version differ.
3. Download the draft's packages and run the checks in
   [PLATFORMS.md](PLATFORMS.md#verification-and-remaining-machine-tests) on real machines.
4. Edit the generated notes and publish the draft. `publish.yml` takes over.

To publish a release again (after adding a channel, say), run **Publish** by hand
with its tag.

## What each release contains

| File | Built on | Signed when |
| --- | --- | --- |
| `Irori-X.Y.Z-linux.tar.gz`, `Irori-X.Y.Z-linux-arm64.tar.gz` | Ubuntu 22.04 (x86-64, arm64) | — |
| `Irori-X.Y.Z-x86_64.AppImage`, `Irori-X.Y.Z-aarch64.AppImage` (and `.AppImage.zsync` for AppImageUpdate) | Ubuntu 22.04 (x86-64, arm64) | — |
| `irori_X.Y.Z-1_amd64.deb`, `irori-sign-in_X.Y.Z-1_amd64.deb` (and `_arm64.deb`) | Ubuntu 22.04 (x86-64, arm64) | The apt repository is signed |
| `irori-X.Y.Z-1.x86_64.rpm`, `irori-sign-in-X.Y.Z-1.x86_64.rpm` (and `.aarch64.rpm`) | Ubuntu 22.04 (x86-64, arm64) | Signed when published to the dnf repository |
| `irori-X.Y.Z-1-x86_64.pkg.tar.zst`, `irori-sign-in-X.Y.Z-1-x86_64.pkg.tar.zst` | Arch Linux (x86-64 only, as Arch is) | Signed when published to the pacman repository |
| `Irori-X.Y.Z-linux.flatpak`, `Irori-X.Y.Z-linux-arm64.flatpak` | GNOME 50 SDK (x86-64, arm64) | — |
| `Irori-X.Y.Z-macos-arm64.dmg` / `-x86_64.dmg` (and `.zip`) | macOS 15 | `MACOS_*` secrets; notarized with the `APPLE_*` secrets too |
| `Irori-X.Y.Z-windows-setup.exe`, `Irori-X.Y.Z-windows.zip`, `Irori-X.Y.Z-windows-portable.zip` | Windows Server 2022 (x64) | SignPath is configured |
| `Irori-X.Y.Z-windows-arm64-setup.exe`, `Irori-X.Y.Z-windows-arm64.zip`, `Irori-X.Y.Z-windows-arm64-portable.zip` | Windows 11 (arm64) | SignPath is configured |

Every architecture builds natively on GitHub's runners (`ubuntu-22.04-arm`,
`windows-11-arm` for arm64). x86-64 files keep the names they had before arm64
builds existed, so AppImageUpdate, the package managers and existing links keep
finding them. The apt and dnf repositories serve both architectures; Scoop and
winget get both Windows builds; the x64 setup also installs on arm64 Windows,
where it runs emulated.

Without signing secrets the workflow still builds every package, unsigned. `SHA256SUMS`
covers every file, and the release job fails when one of the main packages is missing.

The AppImage's tools are pinned in `release.yml`: appimagetool 1.9.1 and the type 2
runtime `20251108`, each checked against its SHA-256. To move to newer ones, update the
URLs and the sums together. The AppImage points AppImageUpdate at the release marked
*latest*, so publishing a release is what offers it to existing AppImages.

## One-time setup

Secrets go under *Settings → Secrets and variables → Actions → Secrets*, variables
under *Variables*. Never put their values in the repository.

### macOS signing and notarization

Needs an Apple Developer Program membership. Only `v*` tags sign and notarize.
Pull requests, branches and manual runs build unsigned packages, so a pushed branch can
never reach the signing keys.

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | Base64 of a *Developer ID Application* certificate and key exported as `.p12` |
| `MACOS_CERTIFICATE_PASSWORD` | That export's password |
| `MACOS_SIGN_IDENTITY` | The certificate's name: `Developer ID Application: Name (TEAMID)` |
| `APPLE_API_KEY_P8` | Base64 of an App Store Connect API key (`AuthKey_….p8`) with the Developer role |
| `APPLE_API_KEY_ID` | That key's ID |
| `APPLE_API_ISSUER_ID` | The issuer ID shown above the keys list |

### Windows signing

[SignPath Foundation](https://signpath.org) signs open-source projects for free.
Apply with the repository, then create a project with the slug `irori`, a signing
policy `release-signing`, and two artifact configurations:
- `executables`: signs `irori.exe` and `irori-sign-in.exe` in a zip;
- `installer`: signs the `*-setup.exe` in a zip.

| Kind | Name | Value |
| --- | --- | --- |
| Secret | `SIGNPATH_API_TOKEN` | A SignPath CI user's API token |
| Variable | `SIGNPATH_ORGANIZATION_ID` | Your SignPath organization ID |

### apt, dnf and pacman repositories

Served at `https://packages.irori.app` (`apt/`, `rpm/` and `arch/`) from this
repository's GitHub Pages (`PACKAGES_URL` in `publish.yml`).

1. Verify `irori.app` for the organization (*Organization settings → Pages →
   Add a domain*, a `TXT` record), so no other account can serve Pages on its
   subdomains.
2. DNS for `irori.app`: a `CNAME` record from `packages` to `irori-app.github.io`,
   not proxied, so GitHub can issue its certificate.
3. *Settings → Pages*: **Source** = **GitHub Actions**, **Custom domain** =
   `packages.irori.app`, then **Enforce HTTPS** once the certificate is issued.
4. *Settings → Environments → github-pages*: allow the `v*` tag pattern besides
   `main`, because `publish.yml` runs from the release's tag.
5. Make a signing key with no passphrase, used only for this. RSA, because
   older `rpm` releases (openSUSE Leap, for one) cannot verify ed25519
   signatures: `gpg --quick-gen-key "Irori packages" rsa4096 sign never`.
   Keep a backup of the private key outside GitHub.
6. Secret `PACKAGES_GPG_PRIVATE_KEY`: `gpg --armor --export-secret-keys <fingerprint>`.
7. Variable `PUBLISH_PACKAGE_REPOSITORIES` = `true`.

Users then follow the instructions on that page (`index.html`). Each release
replaces the repositories, which hold the latest version only.

### AUR (`irori-bin`, `irori-sign-in-bin`)

Off for now: AUR registration is closed, and Arch users install from the pacman
repository above. The `aur` job in `publish.yml` stays ready for when it reopens, and
pull requests check its PKGBUILD with `makepkg --printsrcinfo`.

1. Create an AUR account and add an SSH key to it.
2. Secret `AUR_SSH_PRIVATE_KEY`: that key's private half.
3. Variable `AUR_COMMIT_EMAIL`: the address commits to the AUR use.
4. Variable `PUBLISH_AUR` = `true`.

### Homebrew and Scoop

1. Create the repositories `<owner>/homebrew-tap` and `<owner>/scoop-bucket`.
2. Give each a deploy key with write access (*Settings → Deploy keys*), and store
   the private halves as the secrets `HOMEBREW_TAP_DEPLOY_KEY` and
   `SCOOP_BUCKET_DEPLOY_KEY`. A deploy key can push to its one repository only.
3. Variables `PUBLISH_HOMEBREW` = `true` and `PUBLISH_SCOOP` = `true`.

Users install with `brew install --cask <owner>/tap/irori` and
`scoop bucket add irori https://github.com/<owner>/scoop-bucket && scoop install irori`.

### winget

`release_manifests.py` writes all three manifests of each version (`Irori.Irori`, both
setups, the setup's uninstall key as `ProductCode`) with
`InstallerSwitches: Custom: /CHANNEL=winget`, so the setup writes `Channel=winget` to
`install.ini` and Irori offers `winget upgrade` instead of the release page. The `winget`
job submits them with Microsoft's `wingetcreate submit`, which opens the pull request on
`microsoft/winget-pkgs`, for the first version too; winget's moderators review new
packages before they are listed.

1. Fork `microsoft/winget-pkgs` to the account that owns the token below.
2. Secret `WINGET_TOKEN`: a classic token with the `public_repo` scope.
3. Variable `PUBLISH_WINGET` = `true`.

### Fedora COPR

The `copr` job builds a source package holding both Linux tarballs and the spec
`release_manifests.py` writes (`irori` and `irori-sign-in`, repackaging the release as
the AUR's `irori-bin` does), and sends it to COPR, which builds it for each chroot.

1. Create a COPR project (for example `irori/irori`) with the `fedora-*-x86_64` and
   `fedora-*-aarch64` chroots you want to serve.
2. Secret `COPR_CONFIG`: the contents of the API token file from
   <https://copr.fedorainfracloud.org/api/> (`~/.config/copr`).
3. Variable `COPR_PROJECT`: `<owner>/<project>`.
4. Variable `PUBLISH_COPR` = `true`.

Users install with `sudo dnf copr enable <owner>/<project> && sudo dnf install irori`.

### Flathub

Flathub builds Irori itself from its repository `flathub/app.irori.Irori`. For each
release the `flathub` job takes the manifest `release_manifests.py` writes (the tag's
`packaging/flatpak/app.irori.Irori.yml` with its `dir` source replaced by a `git` source
pinned to the tag and its commit) and a `cargo-sources.json` generated from the tag's
`Cargo.lock`, and opens a pull request there; Flathub's bot builds it, and merging it
publishes.

1. Submit the first version by hand following
   [Flathub's submission guide](https://docs.flathub.org/docs/for-app-authors/submission),
   with the files the `manifests` artifact of a **Publish** run holds
   (`flathub/app.irori.Irori.yml`) and a `cargo-sources.json` made as the job does.
2. Verify the app id: Flathub gives `app.irori.Irori` a token to publish at
   `https://irori.app/.well-known/org.flathub.VerifiedApps.txt`. Add it to
   `website/public/.well-known/` and deploy the site.
3. Secret `FLATHUB_TOKEN`: a token of an account with write access to
   `flathub/app.irori.Irori` (`repo` scope), which Flathub grants the app's maintainers.
4. Variable `PUBLISH_FLATHUB` = `true`.
