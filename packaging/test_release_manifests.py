"""Package-manager manifests carry the release's version and checksums."""
import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

import release_manifests

COMMIT = "0123456789abcdef0123456789abcdef01234567"


class ReleaseManifestTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.dist = Path(temporary.name)
        self.sums = {}
        for name in release_manifests.artifact_names("1.2.3").values():
            data = name.encode()
            (self.dist / name).write_bytes(data)
            self.sums[name] = hashlib.sha256(data).hexdigest()

    def test_every_manifest_names_the_release_and_its_checksums(self):
        out = release_manifests.render("1.2.3", self.dist, "irori-app/irori")
        pkgbuild = out["aur/PKGBUILD"]
        self.assertIn("pkgver=1.2.3\n", pkgbuild)
        self.assertIn(f"sha256sums=('{self.sums['Irori-1.2.3-linux.tar.gz']}')", pkgbuild)
        self.assertIn("releases/download/v$pkgver/Irori-$pkgver-linux.tar.gz", pkgbuild)
        self.assertIn('pkgdesc="Every game you own, around one fire"', pkgbuild)

        cask = out["homebrew/Casks/irori.rb"]
        self.assertIn('version "1.2.3"', cask)
        self.assertIn(f'arm:   "{self.sums["Irori-1.2.3-macos-arm64.dmg"]}"', cask)
        self.assertIn(f'intel: "{self.sums["Irori-1.2.3-macos-x86_64.dmg"]}"', cask)
        self.assertIn("Irori-#{version}-macos-#{arch}.dmg", cask)
        self.assertIn('homepage "https://irori.app"', cask)
        self.assertIn('url="https://irori.app"', pkgbuild)

        scoop = json.loads(out["scoop/bucket/irori.json"])
        self.assertEqual(scoop["version"], "1.2.3")
        self.assertEqual(scoop["homepage"], "https://irori.app")
        self.assertEqual(scoop["architecture"]["64bit"]["hash"], self.sums["Irori-1.2.3-windows.zip"])
        self.assertTrue(scoop["architecture"]["64bit"]["url"].endswith("/v1.2.3/Irori-1.2.3-windows.zip"))
        self.assertEqual(scoop["architecture"]["arm64"]["hash"], self.sums["Irori-1.2.3-windows-arm64.zip"])
        self.assertTrue(scoop["architecture"]["arm64"]["url"].endswith("/v1.2.3/Irori-1.2.3-windows-arm64.zip"))
        self.assertTrue(scoop["autoupdate"]["architecture"]["arm64"]["url"].endswith("Irori-$version-windows-arm64.zip"))
        self.assertEqual(scoop["checkver"], {"github": "https://github.com/irori-app/irori"})

        # Flathub needs the tag's commit, which only publish.yml passes.
        self.assertNotIn("flathub/app.irori.Irori.yml", out)

    def test_copr_spec_repackages_both_tarballs_per_architecture(self):
        spec = release_manifests.render("1.2.3", self.dist, "irori-app/irori")["copr/irori.spec"]
        self.assertIn("Version:        1.2.3\n", spec)
        self.assertIn("Release:        1%{?dist}\n", spec)
        self.assertIn("Source0:        https://github.com/irori-app/irori/releases/download/"
                      "v%{version}/Irori-%{version}-linux.tar.gz\n", spec)
        self.assertIn("Irori-%{version}-linux-arm64.tar.gz\n", spec)
        self.assertIn("ExclusiveArch:  x86_64 aarch64\n", spec)
        self.assertRegex(spec, r"%ifarch aarch64\ntar -xzf %\{SOURCE1\}\n%else\ntar -xzf %\{SOURCE0\}\n%endif")
        # The sign-in window (WebKitGTK) is only recommended, as the .rpm's is.
        self.assertIn("Recommends:     irori-sign-in = %{version}-%{release}", spec)
        self.assertNotRegex(spec, r"(?m)^Requires:\s+irori-sign-in")
        self.assertIn("%package sign-in", spec)
        self.assertIn("%files sign-in\n%license Irori/LICENSE\n%{_bindir}/irori-sign-in\n", spec)
        # Every file installed is listed, and nothing else.
        installed = set(re.findall(r"%\{buildroot\}(%\{_\w+\}/[\w./-]+)", spec))
        listed = set(re.findall(r"(?m)^(%\{_\w+\}/[\w./-]+)$", spec))
        self.assertEqual(installed, listed)

    def test_copr_changelog_has_an_rpm_date_in_english(self):
        spec = release_manifests.spec("1.2.3", "irori-app/irori", datetime.date(2026, 11, 3))
        self.assertIn("%changelog\n* Tue Nov 03 2026 Irori developers - 1.2.3-1\n", spec)

    def test_winget_manifests_carry_the_channel_switch_and_both_setups(self):
        out = release_manifests.render("1.2.3", self.dist, "irori-app/irori")
        folder = "winget/manifests/i/Irori/Irori/1.2.3"
        self.assertEqual(sorted(k for k in out if k.startswith("winget/")), [
            f"{folder}/Irori.Irori.installer.yaml",
            f"{folder}/Irori.Irori.locale.en-US.yaml",
            f"{folder}/Irori.Irori.yaml",
        ])
        for name in ("Irori.Irori.yaml", "Irori.Irori.installer.yaml", "Irori.Irori.locale.en-US.yaml"):
            text = out[f"{folder}/{name}"]
            self.assertIn("PackageIdentifier: Irori.Irori\nPackageVersion: 1.2.3\n", text)
            self.assertIn("ManifestVersion: 1.6.0\n", text)
            self.assertNotIn("\t", text)
        installer = out[f"{folder}/Irori.Irori.installer.yaml"]
        self.assertIn("InstallerType: inno\nScope: user\n", installer)
        # The switch that tells Irori winget updates it (app_update.rs).
        self.assertIn("InstallerSwitches:\n  Custom: /CHANNEL=winget\n", installer)
        self.assertIn("ProductCode: '{6E2B7C1A-0D4F-4C8E-9A35-2F1B8D7E4C60}_is1'", installer)
        for arch, name in (("x64", "Irori-1.2.3-windows-setup.exe"),
                           ("arm64", "Irori-1.2.3-windows-arm64-setup.exe")):
            self.assertIn(f"- Architecture: {arch}\n"
                          f"  InstallerUrl: https://github.com/irori-app/irori/releases/download/v1.2.3/{name}\n"
                          f"  InstallerSha256: {self.sums[name].upper()}\n", installer)
        locale = out[f"{folder}/Irori.Irori.locale.en-US.yaml"]
        self.assertIn("ManifestType: defaultLocale\n", locale)
        self.assertIn("License: MIT\n", locale)
        self.assertIn("DefaultLocale: en-US\n", out[f"{folder}/Irori.Irori.yaml"])

    def test_product_code_follows_the_setups_app_id(self):
        script = self.dist / "irori.iss"
        script.write_text("[Setup]\nAppId={{12345678-ABCD-4000-8000-0123456789AB}\nAppName=Irori\n")
        self.assertEqual(release_manifests.inno_product_code(script),
                         "{12345678-ABCD-4000-8000-0123456789AB}_is1")
        script.write_text("[Setup]\nAppName=Irori\n")
        with self.assertRaisesRegex(ValueError, "No AppId"):
            release_manifests.inno_product_code(script)

    def test_flathub_manifest_pins_the_tag_and_keeps_everything_else(self):
        manifest = release_manifests.FLATPAK_MANIFEST.read_text()
        out = release_manifests.render("1.2.3", self.dist, "irori-app/irori", COMMIT, manifest)
        flathub = out["flathub/app.irori.Irori.yml"]
        self.assertIn("      - type: git\n"
                      "        url: https://github.com/irori-app/irori.git\n"
                      "        tag: v1.2.3\n"
                      f"        commit: {COMMIT}\n"
                      "      - cargo-sources.json\n", flathub)
        self.assertNotIn("type: dir", flathub)
        self.assertNotIn("path: ../..", flathub)
        # The rest is the manifest CI builds, line for line.
        kept = [line for line in manifest.splitlines()
                if line.strip() not in ("- type: dir", "path: ../..", "skip:", "- target", "- dist", "- website")]
        flathub_lines = flathub.splitlines()
        for line in kept:
            self.assertIn(line, flathub_lines)
        self.assertIn("id: app.irori.Irori", flathub_lines)

    def test_flathub_refuses_what_it_cannot_pin(self):
        manifest = release_manifests.FLATPAK_MANIFEST.read_text()
        with self.assertRaisesRegex(ValueError, "commit hash"):
            release_manifests.flathub("1.2.3", "irori-app/irori", "v1.2.3", manifest)
        with self.assertRaisesRegex(ValueError, "exactly one `dir` source"):
            release_manifests.flathub("1.2.3", "irori-app/irori", COMMIT, "modules: []\n")

    def test_arch_repository_pkgbuild_builds_plain_names_from_the_local_tarball(self):
        text = release_manifests.pkgbuild("1.2.3", "irori-app/irori", "abc123", aur=False)
        self.assertIn("pkgname=(irori irori-sign-in)\n", text)
        self.assertIn('source=("Irori-$pkgver-linux.tar.gz")', text)
        self.assertIn("package_irori() {", text)
        self.assertIn("package_irori-sign-in() {", text)
        self.assertIn("conflicts=(irori-bin)", text)
        self.assertNotIn("releases/download", text)

        aur = release_manifests.pkgbuild("1.2.3", "irori-app/irori", "abc123")
        self.assertIn("pkgname=(irori-bin irori-sign-in-bin)\n", aur)
        self.assertIn("provides=(irori)", aur)

    def test_missing_archive_is_named(self):
        for name in ("Irori-1.2.3-macos-x86_64.dmg", "Irori-1.2.3-windows-arm64-setup.exe",
                     "Irori-1.2.3-linux-arm64.tar.gz"):
            (self.dist / name).unlink()
            with self.assertRaisesRegex(FileNotFoundError, re.escape(name)):
                release_manifests.render("1.2.3", self.dist, "irori-app/irori")
            (self.dist / name).write_bytes(name.encode())

    def run_script(self, *args: str) -> tuple[subprocess.CompletedProcess, Path]:
        output = self.dist / "out"
        script = Path(release_manifests.__file__)
        result = subprocess.run([sys.executable, str(script), "--version", "v1.2.3", "--dist", str(self.dist),
                                 "--repo", "irori-app/irori", "--output", str(output), *args],
                                capture_output=True, text=True)
        return result, output

    def test_single_channel_flags_need_only_the_linux_tarballs(self):
        for name in release_manifests.artifact_names("1.2.3").values():
            if "-linux" not in name:
                (self.dist / name).unlink()
        for flag, written in (("--aur", "aur/PKGBUILD"), ("--arch-repository", "arch/PKGBUILD"),
                              ("--copr", "copr/irori.spec")):
            result, output = self.run_script(flag)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual([p.relative_to(output).as_posix() for p in output.rglob("*") if p.is_file()],
                             [written])
            for path in output.rglob("*"):
                if path.is_file():
                    path.unlink()
        (self.dist / "Irori-1.2.3-linux-arm64.tar.gz").unlink()
        result, _ = self.run_script("--copr")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("linux-arm64.tar.gz", result.stderr)

    def test_the_full_run_writes_every_channel(self):
        flatpak = self.dist / "flatpak.yml"
        flatpak.write_text(release_manifests.FLATPAK_MANIFEST.read_text())
        result, output = self.run_script("--commit", COMMIT, "--flatpak-manifest", str(flatpak))
        self.assertEqual(result.returncode, 0, result.stderr)
        written = sorted(p.relative_to(output).as_posix() for p in output.rglob("*") if p.is_file())
        self.assertEqual(written, sorted([
            "aur/PKGBUILD", "copr/irori.spec", "flathub/app.irori.Irori.yml",
            "homebrew/Casks/irori.rb", "scoop/bucket/irori.json",
            "winget/manifests/i/Irori/Irori/1.2.3/Irori.Irori.installer.yaml",
            "winget/manifests/i/Irori/Irori/1.2.3/Irori.Irori.locale.en-US.yaml",
            "winget/manifests/i/Irori/Irori/1.2.3/Irori.Irori.yaml",
        ]))


class WorkflowTests(unittest.TestCase):
    """Each channel has a publishing job gated on its own repository
    variable, and a check that runs on pull requests."""

    root = Path(release_manifests.__file__).resolve().parent.parent

    def test_every_channel_publishes_only_when_switched_on(self):
        publish = (self.root / ".github/workflows/publish.yml").read_text()
        for job, variable in (("aur", "PUBLISH_AUR"), ("scoop", "PUBLISH_SCOOP"), ("winget", "PUBLISH_WINGET"),
                              ("copr", "PUBLISH_COPR"), ("flathub", "PUBLISH_FLATHUB")):
            self.assertRegex(publish, rf"(?m)^  {job}:\n    if: vars\.{variable} == 'true'\n",
                             f"{job} must be gated on {variable}")

    def test_pull_requests_check_every_channel(self):
        release = (self.root / ".github/workflows/release.yml").read_text()
        self.assertIn("bash packaging/copr/build.sh", release)     # COPR
        self.assertIn("bash packaging/arch/build.sh", release)     # AUR's PKGBUILD, read by makepkg
        self.assertIn("'/CHANNEL=winget'", release)                # winget's install
        self.assertIn("packaging/flatpak/app.irori.Irori.yml", release)  # Flathub's manifest
        self.assertIn("--portable", release)                       # Scoop's zip
        build = (self.root / "packaging/arch/build.sh").read_text()
        self.assertIn("--aur", build)
        self.assertIn("makepkg --printsrcinfo", build)


if __name__ == "__main__":
    unittest.main()
