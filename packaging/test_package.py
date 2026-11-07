"""Packaging regressions that can run on every build host."""
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch
import zipfile

import package

# AppDirs hold relative symlinks (`irori.desktop`, `.DirIcon`), which Windows
# can't follow; AppImages are only built on Linux.
needs_posix_links = unittest.skipIf(os.name == "nt", "AppDirs need POSIX symlinks")


def write_pe(path: Path, machine: int = 0x8664) -> None:
    data = bytearray(70)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<H", data, 68, machine)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def write_elf(path: Path, needed: list[str], machine: int = package.ELF_X86_64) -> None:
    """A minimal executable header for `machine` that links `needed`."""
    base, phoff, phnum = 0x400000, 64, 2
    strtab = b"\0" + b"".join(name.encode() + b"\0" for name in needed)
    strtab_offset = phoff + 56 * phnum
    dynamic_offset = strtab_offset + len(strtab)
    names, at = [], 1
    for name in needed:
        names.append(at)
        at += len(name) + 1
    dynamic = b"".join(struct.pack("<qQ", package.DT_NEEDED, offset) for offset in names)
    dynamic += struct.pack("<qQ", package.DT_STRTAB, base + strtab_offset) + struct.pack("<qQ", 0, 0)
    size = dynamic_offset + len(dynamic)
    header = bytearray(64)
    header[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", header, 18, machine)
    struct.pack_into("<Q", header, 32, phoff)
    struct.pack_into("<HH", header, 54, 56, phnum)
    load = struct.pack("<IIQQQQQQ", package.PT_LOAD, 5, 0, base, base, size, size, 0x1000)
    dyn = struct.pack("<IIQQQQQQ", package.PT_DYNAMIC, 6, dynamic_offset, base + dynamic_offset,
                      base + dynamic_offset, len(dynamic), len(dynamic), 8)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(header) + load + dyn + strtab + dynamic)


class LinuxPackagingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.binaries = self.root / "built binaries"
        write_elf(self.binaries / "irori", ["libxkbcommon.so.0", "libc.so.6"])
        write_elf(self.binaries / "irori-sign-in", ["libwebkit2gtk-4.1.so.0", "libgtk-3.so.0", "libc.so.6"])
        self.tool = self.root / "appimagetool"
        self.tool.write_text("")
        self.runtime = self.root / "runtime-x86_64"
        self.runtime.write_text("")
        environment = patch.dict(os.environ, {"IRORI_APPIMAGETOOL": str(self.tool), "IRORI_APPIMAGE_RUNTIME": str(self.runtime)})
        environment.start()
        self.addCleanup(environment.stop)
        self.zsync = True

    def build_appimage(self):
        """Package with a stand-in appimagetool; returns the AppImage, the
        command and what the AppDir held when the tool ran."""
        seen = {}

        def appimagetool(command, check, env, cwd):
            appdir, appimage = Path(command[-2]), Path(command[-1])
            seen["command"], seen["env"] = command, env
            seen["files"] = {str(p.relative_to(appdir)): p for p in appdir.rglob("*")}
            seen["desktop"] = (appdir / "irori.desktop").read_text()
            seen["icon"] = (appdir / ".DirIcon").read_bytes()
            seen["run"] = ((appdir / "AppRun").read_text(), (appdir / "AppRun").stat().st_mode)
            appimage.write_bytes(b"AppImage")
            if self.zsync:
                appimage.with_name(appimage.name + ".zsync").write_text("zsync: 0.6.2\n")
            return subprocess.CompletedProcess(command, 0)

        with patch.object(package.subprocess, "run", side_effect=appimagetool):
            tarball = package.package("linux", self.binaries, self.root / "output", appimage=True)
        return tarball, seen

    def test_elf_needed_reads_the_linked_libraries(self):
        self.assertEqual(package.elf_needed(self.binaries / "irori"), ["libxkbcommon.so.0", "libc.so.6"])

    @unittest.skipUnless(sys.platform == "linux" and Path("/bin/sh").is_file(), "needs a Linux executable")
    def test_elf_needed_reads_a_real_executable(self):
        self.assertIn("libc.so.6", package.elf_needed(Path("/bin/sh").resolve()))

    @needs_posix_links
    def test_appimage_holds_both_programs_desktop_entry_icon_and_metainfo(self):
        tarball, seen = self.build_appimage()
        output = self.root / "output"
        self.assertEqual(tarball, output / f"Irori-{self.version()}-linux.tar.gz")
        appimage = output / f"Irori-{self.version()}-x86_64.AppImage"
        self.assertTrue(appimage.is_file())
        self.assertTrue(os.access(appimage, os.X_OK))
        for name in ("usr/bin/irori", "usr/bin/irori-sign-in", "usr/share/applications/irori.desktop",
                     "usr/share/icons/hicolor/scalable/apps/irori.svg",
                     "usr/share/metainfo/app.irori.Irori.metainfo.xml", "usr/share/doc/irori/LICENSE",
                     "irori.desktop", "irori.svg", ".DirIcon", "AppRun"):
            self.assertIn(name, seen["files"])
        self.assertIn("Exec=irori %u", seen["desktop"])
        self.assertIn(b"<svg", seen["icon"])
        run, mode = seen["run"]
        self.assertTrue(run.startswith("#!/bin/sh\n") and 'exec "$here/usr/bin/irori" "$@"' in run)
        self.assertTrue(mode & 0o111)
        self.assertNotIn("LD_LIBRARY_PATH", run)
        self.assertFalse(any(name.endswith(".so") or ".so." in name for name in seen["files"]))

    @needs_posix_links
    def test_appimage_carries_update_information_and_the_pinned_runtime(self):
        _, seen = self.build_appimage()
        command = seen["command"]
        self.assertEqual(command[0], str(self.tool))
        self.assertEqual(command[command.index("--updateinformation") + 1],
                         "gh-releases-zsync|irori-app|irori|latest|Irori-*-x86_64.AppImage.zsync")
        self.assertEqual(command[command.index("--runtime-file") + 1], str(self.runtime))
        self.assertEqual(seen["env"]["ARCH"], "x86_64")
        self.assertEqual(seen["env"]["VERSION"], self.version())
        self.assertTrue((self.root / f"output/Irori-{self.version()}-x86_64.AppImage.zsync").is_file())

    @needs_posix_links
    def test_arm64_packages_are_named_for_their_architecture(self):
        write_elf(self.binaries / "irori", ["libxkbcommon.so.0", "libc.so.6"], machine=package.ELF_AARCH64)
        write_elf(self.binaries / "irori-sign-in", ["libc.so.6"], machine=package.ELF_AARCH64)
        tarball, seen = self.build_appimage()
        output = self.root / "output"
        self.assertEqual(tarball, output / f"Irori-{self.version()}-linux-arm64.tar.gz")
        self.assertTrue((output / f"Irori-{self.version()}-aarch64.AppImage").is_file())
        self.assertEqual(seen["env"]["ARCH"], "aarch64")
        # An arm64 AppImage never updates itself to the x86-64 one.
        command = seen["command"]
        self.assertEqual(command[command.index("--updateinformation") + 1],
                         "gh-releases-zsync|irori-app|irori|latest|Irori-*-aarch64.AppImage.zsync")

    @needs_posix_links
    def test_update_information_needs_its_zsync_file(self):
        self.zsync = False
        with self.assertRaisesRegex(RuntimeError, "zsyncmake"):
            self.build_appimage()

    def test_irori_must_start_without_webkitgtk(self):
        write_elf(self.binaries / "irori", ["libwebkit2gtk-4.1.so.0", "libc.so.6"])
        with self.assertRaisesRegex(RuntimeError, "without WebKitGTK"):
            self.build_appimage()
        self.assertFalse((self.root / "output").exists())

    def test_mixed_and_other_architectures_are_rejected(self):
        write_elf(self.binaries / "irori-sign-in", ["libc.so.6"], machine=package.ELF_AARCH64)
        with self.assertRaisesRegex(ValueError, "different architectures: aarch64, x86_64"):
            self.build_appimage()
        write_elf(self.binaries / "irori-sign-in", ["libc.so.6"], machine=243)  # RISC-V
        with self.assertRaisesRegex(ValueError, "x86-64 and arm64 only"):
            self.build_appimage()
        (self.binaries / "irori-sign-in").write_text("#!/bin/sh\n")
        with self.assertRaisesRegex(ValueError, "Not a 64-bit Linux executable"):
            self.build_appimage()

    def test_missing_appimagetool_is_explained(self):
        with patch.dict(os.environ, {"IRORI_APPIMAGETOOL": "", "PATH": str(self.root / "absent")}):
            with self.assertRaisesRegex(FileNotFoundError, "appimagetool"):
                self.build_appimage()

    def test_options_belong_to_their_platform(self):
        with self.assertRaisesRegex(ValueError, "--appimage for Linux"):
            package.package("windows", self.binaries, self.root / "output", appimage=True)
        self.assertFalse((self.root / "output").exists())

    def version(self):
        return tomllib.loads((Path(package.__file__).resolve().parent.parent / "Cargo.toml").read_text())["workspace"]["package"]["version"]


class MacPackagingTests(unittest.TestCase):
    def test_info_plist_offers_irori_for_windows_files_without_taking_them_over(self):
        info = package.macos_info("1.2.3")
        self.assertEqual(info["CFBundleURLTypes"][0]["CFBundleURLSchemes"], ["irori"])
        documents = info["CFBundleDocumentTypes"]
        self.assertEqual(len(documents), 1)
        self.assertEqual(documents[0]["LSHandlerRank"], "Alternate")
        self.assertEqual(documents[0]["CFBundleTypeRole"], "Viewer")
        self.assertEqual(documents[0]["CFBundleTypeExtensions"], ["exe", "msi", "bat", "cmd", "reg"])

    def test_windows_file_extensions_match_the_runtime(self):
        source = (Path(package.__file__).resolve().parent.parent / "crates/runtime/src/windows_file.rs").read_text()
        line = next(l for l in source.splitlines() if l.startswith("pub const EXTENSIONS"))
        self.assertEqual(re.findall(r'"([a-z]+)"', line), package.WINDOWS_FILE_EXTENSIONS)


class WindowsPackagingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.binaries = self.root / "built binaries"
        self.runtime = self.root / "redistributable files"
        for name in ("irori.exe", "irori-sign-in.exe"):
            write_pe(self.binaries / name)
        for name in package.WINDOWS_CRT_FILES:
            write_pe(self.runtime / name)
        environment = patch.dict(os.environ, {"IRORI_WINDOWS_CRT_DIR": str(self.runtime)})
        environment.start()
        self.addCleanup(environment.stop)

    def test_windows_archive_contains_runtime_next_to_both_executables(self):
        archive = package.package("windows", self.binaries, self.root / "output")
        with zipfile.ZipFile(archive) as zipped:
            for name in ("irori.exe", "irori-sign-in.exe", "vcruntime140.dll", "vcruntime140_1.dll", "install.ps1"):
                self.assertIn("Irori/" + name, zipped.namelist())
            self.assertEqual(zipped.read("Irori/vcruntime140.dll"), (self.runtime / "vcruntime140.dll").read_bytes())

    def test_portable_zip_keeps_data_beside_irori_and_installs_nothing(self):
        output = self.root / "output"
        installable = package.package("windows", self.binaries, output, portable=True)
        portable = next(output.glob("Irori-*-windows-portable.zip"))
        with zipfile.ZipFile(portable) as zipped:
            names = zipped.namelist()
            for name in ("irori.exe", "irori-sign-in.exe", "vcruntime140.dll", "vcruntime140_1.dll", "LICENSE.txt"):
                self.assertIn("Irori-portable/" + name, names)
            self.assertIn("leaves the rest of Windows alone", zipped.read("Irori-portable/portable").decode())
            self.assertNotIn("Irori-portable/install.ps1", names)
            self.assertTrue(all(name.startswith("Irori-portable/") for name in names))
        with zipfile.ZipFile(installable) as zipped:
            self.assertNotIn("Irori/portable", zipped.namelist())
        # Inno Setup builds the setup from this folder.
        self.assertFalse((output / "Irori/portable").exists())

    def test_missing_runtime_fails_before_creating_output(self):
        (self.runtime / "vcruntime140_1.dll").unlink()
        with self.assertRaisesRegex(FileNotFoundError, "redistributable DLLs"):
            package.package("windows", self.binaries, self.root / "output")
        self.assertFalse((self.root / "output").exists())

    def test_arm_runtime_cannot_be_bundled_with_x64_application(self):
        write_pe(self.runtime / "vcruntime140.dll", package.PE_ARM64)
        with self.assertRaisesRegex(ValueError, "vcruntime140.dll is arm64, but the package is x64"):
            package.package("windows", self.binaries, self.root / "output")
        self.assertFalse((self.root / "output").exists())

    def test_x86_application_is_rejected(self):
        write_pe(self.binaries / "irori.exe", 0x014C)
        with self.assertRaisesRegex(ValueError, "x64 and arm64 only"):
            package.package("windows", self.binaries, self.root / "output")

    def test_arm64_packages_are_named_for_their_architecture(self):
        for file in [*self.binaries.iterdir(), self.runtime / "vcruntime140.dll"]:
            write_pe(file, package.PE_ARM64)
        # Visual Studio's arm64 folder holds an x64 vcruntime140_1.dll, for
        # emulated programs; arm64 ones never import it.
        output = self.root / "output"
        archive = package.package("windows", self.binaries, output, portable=True)
        self.assertEqual(archive.name, f"Irori-{self.version()}-windows-arm64.zip")
        portable = output / f"Irori-{self.version()}-windows-arm64-portable.zip"
        self.assertTrue(portable.is_file())
        for path, folder in ((archive, "Irori"), (portable, "Irori-portable")):
            with zipfile.ZipFile(path) as zipped:
                names = zipped.namelist()
                self.assertIn(f"{folder}/irori.exe", names)
                self.assertIn(f"{folder}/vcruntime140.dll", names)
                self.assertNotIn(f"{folder}/vcruntime140_1.dll", names)

    def test_arm64_application_cannot_be_bundled_with_x64_runtime(self):
        for name in ("irori.exe", "irori-sign-in.exe"):
            write_pe(self.binaries / name, package.PE_ARM64)
        with self.assertRaisesRegex(ValueError, "is x64, but the package is arm64"):
            package.package("windows", self.binaries, self.root / "output")

    def test_invalid_runtime_binary_is_rejected(self):
        (self.runtime / "vcruntime140.dll").write_text("not a DLL")
        with self.assertRaisesRegex(ValueError, "Not a Windows executable"):
            package.package("windows", self.binaries, self.root / "output")

    def test_developer_prompt_redist_directory_is_discovered(self):
        root = self.root / "toolchain redist"
        expected = root / "x64/Microsoft.VC143.CRT"
        for name in package.WINDOWS_CRT_FILES:
            write_pe(expected / name)
        arm = root / "arm64/Microsoft.VC143.CRT"
        for name in package.WINDOWS_ARM64_CRT_FILES:
            write_pe(arm / name, package.PE_ARM64)
        with patch.dict(os.environ, {"IRORI_WINDOWS_CRT_DIR": "", "VCToolsRedistDir": str(root), "ProgramFiles(x86)": str(self.root / "absent") }):
            self.assertEqual(package.windows_crt_directory(), expected)
            self.assertEqual(package.windows_crt_directory("arm64"), arm)

    def version(self):
        return tomllib.loads((Path(package.__file__).resolve().parent.parent / "Cargo.toml").read_text())["workspace"]["package"]["version"]


if __name__ == "__main__":
    unittest.main()
