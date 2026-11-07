#!/usr/bin/env python3
"""Package already-built Irori binaries using only Python's standard library.

    cargo build --locked --release -p irori -p irori-sign-in
    python3 packaging/package.py --platform macos --binary-dir target/release --output dist

`--appimage` (Linux) also writes an AppImage with appimagetool, and `--portable`
(Windows) a zip whose Irori keeps everything in its own folder.

The architecture is read from the binaries: x86-64 or arm64. x86-64 packages
keep their plain names (`Irori-X.Y.Z-linux.tar.gz`, `Irori-X.Y.Z-windows.zip`),
so the links and package managers made for them keep working; arm64 ones add
`-arm64` (`-aarch64` for the AppImage, as AppImages are named).

Signing is deliberately separate: use a Developer ID for public macOS releases
and Authenticode for Windows. CI packages are unsigned test artifacts.
"""
import argparse
import os
from pathlib import Path
import plistlib
import shutil
import stat
import struct
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile


def validate_macos_dependencies(binary: Path) -> None:
    """Fail rather than ship an app that only runs on its build machine."""
    linked = subprocess.run(["otool", "-L", str(binary)], check=True, capture_output=True, text=True)
    for line in linked.stdout.splitlines():
        if not line.startswith("\t"):
            continue
        dependency = line.strip().split(" (compatibility version", 1)[0]
        if not dependency.startswith(("/System/Library/", "/usr/lib/")):
            raise RuntimeError(
                f"{binary.name} depends on a library outside macOS: {dependency}. "
                "Enable static linking or explicitly bundle and relocate that dependency."
            )


def macos_arch(binary: Path) -> str:
    """`arm64`, `x86_64` or `universal`, read from the binary rather than the build host."""
    archs = subprocess.run(["lipo", "-archs", str(binary)], check=True, capture_output=True, text=True).stdout.split()
    return "universal" if len(archs) > 1 else archs[0]


# Windows files Irori opens in a game's Wine prefix
# (`irori_runtime::windows_file::EXTENSIONS`).
WINDOWS_FILE_EXTENSIONS = ["exe", "msi", "bat", "cmd", "reg"]


def macos_info(version: str) -> dict:
    """Info.plist of Irori.app: the `irori://` scheme, and Windows files
    offered under Finder's "Open With" ("Alternate": Irori never takes them
    over from the user's current app)."""
    return {
        "CFBundleName": "Irori", "CFBundleDisplayName": "Irori",
        "CFBundleIdentifier": "app.irori.Irori", "CFBundleExecutable": "irori",
        "CFBundlePackageType": "APPL", "CFBundleVersion": version,
        "CFBundleShortVersionString": version, "LSMinimumSystemVersion": "12.0",
        "LSApplicationCategoryType": "public.app-category.games",
        "NSHighResolutionCapable": True,
        "NSPrincipalClass": "NSApplication",
        "CFBundleURLTypes": [{"CFBundleURLName": "Irori game launcher",
            "CFBundleTypeRole": "Viewer", "CFBundleURLSchemes": ["irori"]}],
        "CFBundleDocumentTypes": [{"CFBundleTypeName": "Windows program",
            "CFBundleTypeRole": "Viewer", "LSHandlerRank": "Alternate",
            "CFBundleTypeExtensions": WINDOWS_FILE_EXTENSIONS}],
    }


def make_dmg(app: Path, dmg: Path, identity: str) -> None:
    """A compressed disk image showing the app beside a link to /Applications."""
    with tempfile.TemporaryDirectory() as temporary:
        folder = Path(temporary) / "Irori"
        folder.mkdir()
        subprocess.run(["ditto", str(app), str(folder / app.name)], check=True)
        (folder / "Applications").symlink_to("/Applications")
        subprocess.run(["hdiutil", "create", "-quiet", "-volname", "Irori", "-srcfolder", str(folder),
                        "-fs", "HFS+", "-format", "UDZO", str(dmg)], check=True)
    if identity != "-":
        subprocess.run(["codesign", "--force", "--sign", identity, "--timestamp", str(dmg)], check=True)


# The Visual C++ runtime Irori's executables import. vcruntime140_1.dll
# holds x64's exception handling only: arm64 programs never import it, and
# the copy in Visual Studio's arm64 redistributable folder is itself x64
# (for emulated programs), so arm64 packages ship without it.
WINDOWS_CRT_FILES = ("vcruntime140.dll", "vcruntime140_1.dll")
WINDOWS_ARM64_CRT_FILES = ("vcruntime140.dll",)


def windows_crt_files(arch: str) -> tuple[str, ...]:
    """The redistributable DLLs a package for `arch` bundles."""
    return WINDOWS_ARM64_CRT_FILES if arch == "arm64" else WINDOWS_CRT_FILES

# Read by `irori_core::paths::portable_root`; its text is for whoever opens it.
PORTABLE_MARKER = "portable"
PORTABLE_NOTE = """\
While this file is here, Irori keeps its settings, logins, downloads, Wine
builds and caches in this folder, and leaves the rest of Windows alone: no
Start menu shortcut, no irori:// links, no registry entries.

Logins are encrypted with a key kept in this Windows account's Credential
Manager, so on another computer you sign in to your stores again.

Delete this file to use the usual folders in your user profile instead.
"""

# The AppImage bundles no libraries, so nothing leaks into the games Irori
# starts through LD_LIBRARY_PATH. Irori uses the system's GPU and window
# libraries, as the tarball does, and the sign-in window the system's
# WebKitGTK: without it the window fails to start and Irori signs in through
# the browser instead.
APPIMAGE_RUN = """\
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/irori" "$@"
"""
# Libraries Irori must start without (AGENTS.md, "WebKitGTK must stay optional").
SIGN_IN_ONLY_LIBRARIES = ("libwebkit2gtk", "libjavascriptcoregtk", "libgtk-3", "libsoup")

# ELF machine numbers, and the names Linux gives those architectures
# (`uname -m`, which AppImages and appimagetool's `ARCH` use too).
ELF_X86_64, ELF_AARCH64 = 62, 183
LINUX_ARCHES = {ELF_X86_64: "x86_64", ELF_AARCH64: "aarch64"}
PT_LOAD, PT_DYNAMIC = 1, 2
DT_NEEDED, DT_STRTAB = 1, 5


def elf_machine(binary: Path) -> int:
    """The machine a 64-bit little-endian ELF executable is built for."""
    with binary.open("rb") as file:
        header = file.read(20)
    if len(header) < 20 or header[:4] != b"\x7fELF" or header[4] != 2 or header[5] != 1:
        raise ValueError(f"Not a 64-bit Linux executable: {binary}")
    return struct.unpack_from("<H", header, 18)[0]


def elf_needed(binary: Path) -> list[str]:
    """The libraries a 64-bit little-endian ELF executable links (`DT_NEEDED`)."""
    elf_machine(binary)
    data = binary.read_bytes()
    phoff, = struct.unpack_from("<Q", data, 32)
    phentsize, phnum = struct.unpack_from("<HH", data, 54)
    loads, dynamic = [], None
    for index in range(phnum):
        kind, _, offset, address, _, size = struct.unpack_from("<IIQQQQ", data, phoff + index * phentsize)
        if kind == PT_LOAD:
            loads.append((address, offset, size))
        elif kind == PT_DYNAMIC:
            dynamic = (offset, size)
    if dynamic is None:
        return []
    entries = [struct.unpack_from("<qQ", data, dynamic[0] + at) for at in range(0, dynamic[1], 16)]
    strtab = next((value for tag, value in entries if tag == DT_STRTAB), None)
    base = next((offset + strtab - address for address, offset, size in loads
                 if strtab is not None and address <= strtab < address + size), None)
    if base is None:
        raise ValueError(f"No dynamic string table in {binary}")
    return [data[base + value:data.index(b"\0", base + value)].decode() for tag, value in entries if tag == DT_NEEDED]


def validate_linux(binaries: list[Path]) -> str:
    """The architecture both programs are built for (`x86_64` or `aarch64`).
    Never mix two in one package, nor let Irori itself depend on the sign-in
    window's libraries."""
    arches = set()
    for binary in binaries:
        machine = elf_machine(binary)
        if machine not in LINUX_ARCHES:
            raise ValueError(f"Linux packages are for x86-64 and arm64 only: {binary}")
        arches.add(LINUX_ARCHES[machine])
        if binary.name == "irori":
            for library in elf_needed(binary):
                if library.startswith(SIGN_IN_ONLY_LIBRARIES):
                    raise RuntimeError(f"irori links {library}, so it would not start without WebKitGTK. "
                                       "Only irori-sign-in may link it.")
    if len(arches) > 1:
        raise ValueError(f"irori and irori-sign-in are built for different architectures: {', '.join(sorted(arches))}")
    return arches.pop()


def appimage_update_information(arch: str) -> str:
    """Where AppImageUpdate finds new versions: the GitHub release marked
    latest, and its AppImage for the same architecture."""
    return f"gh-releases-zsync|irori-app|irori|latest|Irori-*-{arch}.AppImage.zsync"


def appimagetool_command(appdir: Path, appimage: Path, arch: str) -> list[str]:
    """appimagetool's command line. `IRORI_APPIMAGETOOL` names the tool (else
    `appimagetool` on PATH) and `IRORI_APPIMAGE_RUNTIME` a pinned runtime
    (else the tool downloads its own)."""
    tool = os.environ.get("IRORI_APPIMAGETOOL") or shutil.which("appimagetool")
    if not tool:
        raise FileNotFoundError("Install appimagetool or set IRORI_APPIMAGETOOL to build the AppImage.")
    # zstd in 128 KiB blocks, appimagetool's default: smaller than its xz and
    # quicker to start than larger blocks (appimagetool issue 64).
    command = [tool, "--no-appstream", "--comp", "zstd", "--updateinformation", appimage_update_information(arch)]
    if runtime := os.environ.get("IRORI_APPIMAGE_RUNTIME"):
        command += ["--runtime-file", runtime]
    return [*command, str(appdir), str(appimage)]


def make_appimage(root: Path, binaries: list[Path], output: Path, version: str, arch: str) -> Path:
    """`Irori-X.Y.Z-<arch>.AppImage` and, for AppImageUpdate, its `.zsync`."""
    appimage = output / f"Irori-{version}-{arch}.AppImage"
    if appimage.exists():
        raise FileExistsError(f"Never overwriting {appimage}")
    with tempfile.TemporaryDirectory() as temporary:
        appdir = Path(temporary) / "Irori.AppDir"
        for directory in ("usr/bin", "usr/share/applications", "usr/share/icons/hicolor/scalable/apps",
                          "usr/share/metainfo", "usr/share/doc/irori"):
            (appdir / directory).mkdir(parents=True)
        for binary in binaries:
            shutil.copy2(binary, appdir / "usr/bin" / binary.name)
        shutil.copy2(root / "packaging/irori.desktop", appdir / "usr/share/applications/irori.desktop")
        shutil.copy2(root / "packaging/irori.svg", appdir / "usr/share/icons/hicolor/scalable/apps/irori.svg")
        shutil.copy2(root / "packaging/linux/app.irori.Irori.metainfo.xml", appdir / "usr/share/metainfo")
        shutil.copy2(root / "LICENSE", appdir / "usr/share/doc/irori/LICENSE")
        # appimagetool and desktop integrations read these from the AppDir's top.
        (appdir / "irori.desktop").symlink_to("usr/share/applications/irori.desktop")
        (appdir / "irori.svg").symlink_to("usr/share/icons/hicolor/scalable/apps/irori.svg")
        (appdir / ".DirIcon").symlink_to("irori.svg")
        run = appdir / "AppRun"
        run.write_text(APPIMAGE_RUN)
        run.chmod(0o755)
        environment = {**os.environ, "ARCH": arch, "VERSION": version,
                       # The tool is an AppImage itself; containers have no FUSE.
                       "APPIMAGE_EXTRACT_AND_RUN": "1"}
        subprocess.run(appimagetool_command(appdir, appimage, arch), check=True, env=environment, cwd=output)
    if not appimage.is_file():
        raise RuntimeError(f"appimagetool did not write {appimage}")
    if not appimage.with_name(appimage.name + ".zsync").is_file():
        raise RuntimeError(f"appimagetool wrote no {appimage.name}.zsync, so AppImageUpdate would find "
                           "nothing. Install zsync (zsyncmake) and package again.")
    appimage.chmod(appimage.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return appimage


def make_portable_zip(staging: Path, archive: Path) -> None:
    """The Windows files without the installer script, plus the marker that
    keeps Irori's data beside it. Its folder has its own name, so unpacking
    it over the installable zip's folder can't make that one portable."""
    folder = "Irori-portable"
    with zipfile.ZipFile(archive, "x", zipfile.ZIP_DEFLATED) as zip_file:
        for file in sorted(staging.rglob("*")):
            if file.is_file() and file.name != "install.ps1":
                zip_file.write(file, f"{folder}/{file.relative_to(staging).as_posix()}")
        zip_file.writestr(f"{folder}/{PORTABLE_MARKER}", PORTABLE_NOTE)


# PE machine numbers, and the names Windows and its toolchain give those
# architectures (the Visual C++ redistributable's folders, Inno Setup's).
PE_X64, PE_ARM64 = 0x8664, 0xAA64
WINDOWS_ARCHES = {PE_X64: "x64", PE_ARM64: "arm64"}
# The Visual Studio component that installs each architecture's compiler and
# its redistributable.
WINDOWS_TOOLS_COMPONENT = {
    "x64": "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
    "arm64": "Microsoft.VisualStudio.Component.VC.Tools.ARM64",
}


def windows_arch(binary: Path) -> str:
    """`x64` or `arm64`, read from a Windows executable or DLL."""
    with binary.open("rb") as file:
        dos = file.read(64)
        if len(dos) != 64 or dos[:2] != b"MZ":
            raise ValueError(f"Not a Windows executable: {binary}")
        file.seek(struct.unpack_from("<I", dos, 60)[0])
        pe = file.read(6)
    if len(pe) != 6 or pe[:4] != b"PE\0\0":
        raise ValueError(f"Not a Windows executable: {binary}")
    machine = struct.unpack_from("<H", pe, 4)[0]
    if machine not in WINDOWS_ARCHES:
        raise ValueError(f"Windows packages are for x64 and arm64 only: {binary}")
    return WINDOWS_ARCHES[machine]


def validate_windows(binaries: list[Path], arch: str) -> None:
    """Never mix architectures, as an x64 Irori with an arm64 redistributable."""
    for binary in binaries:
        if (found := windows_arch(binary)) != arch:
            raise ValueError(f"{binary.name} is {found}, but the package is {arch}: {binary}")


def windows_crt_directory(arch: str = "x64") -> Path:
    """Find the installed toolchain's redistributable for `arch`, never
    arbitrary system DLLs."""
    explicit = os.environ.get("IRORI_WINDOWS_CRT_DIR")
    if explicit:
        candidates = [Path(explicit)]
    else:
        roots = []
        if directory := os.environ.get("VCToolsRedistDir"):
            roots.append(Path(directory))
        vswhere = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
        if vswhere.is_file():
            installation = subprocess.run(
                [str(vswhere), "-latest", "-products", "*", "-requires",
                 WINDOWS_TOOLS_COMPONENT[arch], "-property", "installationPath"],
                check=True, capture_output=True, text=True,
            ).stdout.strip()
            if installation:
                redist = Path(installation) / "VC/Redist/MSVC"
                versions = [p for p in redist.glob("*") if p.is_dir()]
                versions.sort(key=lambda p: tuple(int(x) for x in p.name.split(".") if x.isdecimal()), reverse=True)
                roots.extend(versions)
        candidates = [candidate for root in roots for candidate in root.glob(f"{arch}/Microsoft.VC*.CRT")]
    for candidate in candidates:
        if all((candidate / name).is_file() for name in windows_crt_files(arch)):
            return candidate
    raise FileNotFoundError(
        f"Missing {arch} Visual C++ redistributable DLLs. Install the Visual Studio C++ tools for {arch}, "
        f"or set IRORI_WINDOWS_CRT_DIR to their {arch} Microsoft.VC*.CRT directory."
    )


def name_suffix(arch: str) -> str:
    """What package names add for an architecture: nothing for x86-64, whose
    names predate arm64 packages and are what links and manifests point at."""
    return "" if arch in ("x86_64", "x64") else "-arm64"


def package(platform: str, binary_dir: Path, output: Path, dmg: bool = False,
            appimage: bool = False, portable: bool = False) -> Path:
    """Package the binaries and return the archive. Next to it, `dmg` also writes
    a macOS disk image, `appimage` a Linux AppImage and `portable` a portable
    Windows zip."""
    if dmg and platform != "macos" or appimage and platform != "linux" or portable and platform != "windows":
        raise ValueError("--dmg is for macOS, --appimage for Linux and --portable for Windows")
    root = Path(__file__).resolve().parent.parent
    version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    suffix = ".exe" if platform == "windows" else ""
    binaries = [binary_dir / (name + suffix) for name in ("irori", "irori-sign-in")]
    for binary in binaries:
        if not binary.is_file():
            raise FileNotFoundError(f"Build both Irori and its sign-in helper first: {binary}")
    if platform == "macos":
        for binary in binaries:
            validate_macos_dependencies(binary)
    linux_arch = validate_linux(binaries) if platform == "linux" else None
    windows_runtime = []
    if platform == "windows":
        arch = windows_arch(binaries[0])
        runtime_directory = windows_crt_directory(arch)
        windows_runtime = [runtime_directory / name for name in windows_crt_files(arch)]
        validate_windows([*binaries, *windows_runtime], arch)
    output.mkdir(parents=True, exist_ok=True)
    staging = output / ("Irori.app" if platform == "macos" else "Irori")
    staging.mkdir()  # Never overwrite an existing package or unrelated files.
    destination = staging / "Contents/MacOS" if platform == "macos" else staging
    destination.mkdir(parents=True, exist_ok=True)
    for binary in binaries:
        shutil.copy2(binary, destination / binary.name)
    if platform == "macos":
        resources = staging / "Contents/Resources"
        resources.mkdir()
        shutil.copy2(root / "packaging/irori.svg", resources)
        info = macos_info(version)
        icon = root / "packaging/irori.icns"
        if icon.is_file():
            shutil.copy2(icon, resources)
            info["CFBundleIconFile"] = "irori.icns"
        with (staging / "Contents/Info.plist").open("wb") as file:
            plistlib.dump(info, file)
        # Sign the nested helper before the bundle. Signing CFBundleExecutable
        # directly promotes the operation to the whole app, so it must not run
        # while the helper is still unsigned (as it is on Intel builds).
        # Ad-hoc signatures are for local/CI validation, not distribution.
        identity = os.environ.get("IRORI_SIGN_IDENTITY", "-")
        for path in [destination / "irori-sign-in", staging]:
            command = ["codesign", "--force", "--sign", identity]
            if identity != "-":
                command += ["--options", "runtime", "--timestamp", "--entitlements", str(root / "packaging/macos.entitlements")]
            subprocess.run([*command, str(path)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(staging)], check=True)
        arch = macos_arch(destination / "irori")
        archive = output / f"Irori-{version}-macos-{arch}.zip"
        subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(staging), str(archive)], check=True)
        if dmg:
            make_dmg(staging, output / f"Irori-{version}-macos-{arch}.dmg", identity)
    elif platform == "windows":
        for library in windows_runtime:
            shutil.copy2(library, staging / library.name)
        shutil.copy2(root / "packaging/install-windows.ps1", staging / "install.ps1")
        shutil.copy2(root / "packaging/irori.svg", staging)
        shutil.copy2(root / "packaging/irori.ico", staging)
        shutil.copy2(root / "LICENSE", staging / "LICENSE.txt")
        archive = output / f"Irori-{version}-windows{name_suffix(arch)}.zip"
        with zipfile.ZipFile(archive, "x", zipfile.ZIP_DEFLATED) as zip_file:
            for file in staging.rglob("*"):
                if file.is_file():
                    zip_file.write(file, file.relative_to(output))
        if portable:
            make_portable_zip(staging, output / f"Irori-{version}-windows{name_suffix(arch)}-portable.zip")
    else:
        shutil.copy2(root / "packaging/irori.desktop", staging)
        shutil.copy2(root / "packaging/irori.svg", staging)
        shutil.copy2(root / "packaging/linux/app.irori.Irori.metainfo.xml", staging)
        shutil.copy2(root / "LICENSE", staging)
        archive = output / f"Irori-{version}-linux{name_suffix(linux_arch)}.tar.gz"
        with tarfile.open(archive, "x:gz") as tar:
            tar.add(staging, arcname="Irori")
        if appimage:
            make_appimage(root, binaries, output, version, linux_arch)
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=["linux", "macos", "windows"], required=True)
    parser.add_argument("--binary-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--output", type=Path, default=Path("dist"))
    parser.add_argument("--dmg", action="store_true", help="macOS: also build a disk image")
    parser.add_argument("--appimage", action="store_true", help="Linux: also build an AppImage (needs appimagetool)")
    parser.add_argument("--portable", action="store_true", help="Windows: also build the portable zip")
    args = parser.parse_args()
    print(package(args.platform, args.binary_dir.resolve(), args.output.resolve(), args.dmg, args.appimage, args.portable))
