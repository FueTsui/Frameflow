#!/usr/bin/env python3
"""Validate and package a prebuilt macOS executable on any host.

Requires Python 3.11+ and Pillow. This script does not
compile, notarize, or add a Developer ID signature to the executable.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import plistlib
import shutil
import stat
import struct
import subprocess
import tempfile
import tomllib
import zipfile
from pathlib import Path

TARGETS = {
    "aarch64-apple-darwin": (0x0100000C, "arm64"),
    "x86_64-apple-darwin": (0x01000007, "x64"),
}
MINIMUM_OS = (11, 0, 0)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def version_tuple(value: int) -> tuple[int, int, int]:
    return value >> 16, (value >> 8) & 255, value & 255


def version_text(value: tuple[int, int, int]) -> str:
    return ".".join(str(part) for part in value)


def c_string(data: bytes, offset: int, end: int) -> str:
    require(0 <= offset < end <= len(data), "Invalid Mach-O string offset")
    terminator = data.find(b"\0", offset, end)
    require(terminator >= 0, "Unterminated Mach-O string")
    return data[offset:terminator].decode("utf-8")


def verify_adhoc_signature(data: bytes, offset: int, size: int) -> dict:
    """Verify the CodeDirectory and every signed page, as produced by ld64.lld."""
    require(offset > 0 and size >= 12 and offset + size <= len(data), "Invalid code-signature range")
    signature = data[offset : offset + size]
    magic, length, count = struct.unpack_from(">III", signature)
    require(magic == 0xFADE0CC0, "Expected an embedded code-signature SuperBlob")
    require(12 + count * 8 <= length <= size, "Invalid signature SuperBlob length")
    directories = []
    for index in range(count):
        slot, start = struct.unpack_from(">II", signature, 12 + index * 8)
        require(12 + count * 8 <= start <= length - 8, "Invalid signature blob offset")
        blob_magic, blob_length = struct.unpack_from(">II", signature, start)
        require(blob_length >= 8 and start + blob_length <= length, "Invalid signature blob length")
        if slot == 0 or 0x1000 <= slot <= 0x1005:
            require(blob_magic == 0xFADE0C02 and blob_length >= 44, "Invalid CodeDirectory")
            directories.append((slot, signature[start : start + blob_length]))
    require(any(slot == 0 for slot, _ in directories), "Missing primary CodeDirectory")
    result = []
    for slot, directory in directories:
        _, length, version, flags, hash_offset, identifier_offset, special_count, page_count, code_limit = struct.unpack_from(
            ">9I", directory
        )
        hash_size, hash_type, _, page_exponent = struct.unpack_from(">4B", directory, 36)
        require(flags & 2 != 0, "The prebuilt executable must have an ad-hoc code signature")
        require(page_exponent <= 24, "Unsupported signature page size")
        if code_limit == 0xFFFFFFFF and version >= 0x20300:
            require(length >= 64, "Truncated 64-bit CodeDirectory")
            code_limit = struct.unpack_from(">Q", directory, 56)[0]
        require(code_limit == offset, "Signature does not cover all bytes before LC_CODE_SIGNATURE")
        if version >= 0x20100:
            require(length >= 48, "Truncated CodeDirectory")
            require(struct.unpack_from(">I", directory, 44)[0] == 0, "Scatter signatures are not supported")
        algorithms = {1: ("sha1", 20), 2: ("sha256", 32), 3: ("sha256", 20), 4: ("sha384", 48)}
        require(hash_type in algorithms, "Unsupported CodeDirectory hash algorithm")
        algorithm, expected_hash_size = algorithms[hash_type]
        require(hash_size == expected_hash_size, "Invalid CodeDirectory hash size")
        require(hash_offset >= 44 + special_count * hash_size, "Invalid special-slot hash range")
        require(hash_offset + page_count * hash_size <= length, "Code-page hashes exceed CodeDirectory")
        # The packager generates a new Info.plist; accepting a pre-existing seal
        # of an external bundle would invalidate that seal. Linker ad-hoc signing
        # does not contain these special slots.
        special_hashes = directory[hash_offset - special_count * hash_size : hash_offset]
        require(not any(special_hashes), "Prebuilt signature seals external bundle resources; package an ld64.lld ad-hoc executable")
        page_size = (1 << page_exponent) if page_exponent else code_limit
        require(page_size > 0 and page_count == math.ceil(code_limit / page_size), "Invalid signed page count")
        for page in range(page_count):
            contents = data[page * page_size : min((page + 1) * page_size, code_limit)]
            expected = hashlib.new(algorithm, contents).digest()[:hash_size]
            actual = directory[hash_offset + page * hash_size : hash_offset + (page + 1) * hash_size]
            require(actual == expected, f"Code signature mismatch on page {page}")
        result.append({"slot": slot, "identifier": c_string(directory, identifier_offset, length), "algorithm": algorithm, "pages_verified": page_count})
    return {"kind": "ad-hoc", "directories": result}


def inspect_macho(data: bytes, target: str) -> dict:
    require(len(data) >= 32 and data[:4] == b"\xcf\xfa\xed\xfe", "Input must be a real little-endian 64-bit Mach-O executable")
    _, cpu, _, file_type, count, command_bytes, _, _ = struct.unpack_from("<8I", data)
    require(cpu == TARGETS[target][0], "Mach-O CPU architecture does not match --target")
    require(file_type == 2, "Mach-O must be MH_EXECUTE, not an object or library")
    require(32 + command_bytes <= len(data), "Mach-O load commands exceed the file")
    cursor = 32
    minimum = None
    sdk = None
    libraries = []
    signature = None
    entry_point = False
    executable_segment = False
    for _ in range(count):
        require(cursor + 8 <= 32 + command_bytes, "Truncated Mach-O command header")
        command, size = struct.unpack_from("<II", data, cursor)
        require(size >= 8 and size % 8 == 0 and cursor + size <= 32 + command_bytes, "Invalid Mach-O command size")
        if command == 0x32:  # LC_BUILD_VERSION
            require(size >= 24, "Truncated LC_BUILD_VERSION")
            platform, min_value, sdk_value = struct.unpack_from("<III", data, cursor + 8)
            require(platform == 1, "Executable targets an Apple platform other than macOS")
            minimum, sdk = version_tuple(min_value), version_tuple(sdk_value)
        elif command == 0x24:  # LC_VERSION_MIN_MACOSX
            require(size >= 16, "Truncated LC_VERSION_MIN_MACOSX")
            min_value, sdk_value = struct.unpack_from("<II", data, cursor + 8)
            minimum, sdk = version_tuple(min_value), version_tuple(sdk_value)
        elif command in (0xC, 0x80000018, 0x8000001F, 0x20, 0x80000023):
            require(size >= 24, "Truncated dylib command")
            name_offset = struct.unpack_from("<I", data, cursor + 8)[0]
            require(name_offset >= 24, "Invalid dylib name offset")
            name = c_string(data, cursor + name_offset, cursor + size)
            require(name.startswith(("/usr/lib/", "/System/Library/")), f"Non-system dynamic library is not bundled: {name}")
            libraries.append(name)
        elif command == 0x1D:  # LC_CODE_SIGNATURE
            require(size >= 16 and signature is None, "Invalid or duplicate LC_CODE_SIGNATURE")
            signature = struct.unpack_from("<II", data, cursor + 8)
        elif command == 0x80000028:  # LC_MAIN
            require(size >= 24, "Truncated LC_MAIN")
            entry_offset = struct.unpack_from("<Q", data, cursor + 8)[0]
            require(0 < entry_offset < len(data), "Invalid executable entry point")
            entry_point = True
        elif command == 0x19:  # LC_SEGMENT_64
            require(size >= 72, "Truncated LC_SEGMENT_64")
            file_offset, file_size = struct.unpack_from("<QQ", data, cursor + 40)
            initial_protection = struct.unpack_from("<I", data, cursor + 60)[0]
            require(file_offset + file_size <= len(data), "Mach-O segment exceeds file")
            executable_segment |= bool(initial_protection & 4 and file_size > 0)
        cursor += size
    require(cursor == 32 + command_bytes, "Mach-O load command sizes are inconsistent")
    require(minimum is not None and minimum <= MINIMUM_OS, "Executable must declare a deployment target no newer than macOS 11.0")
    require(entry_point and executable_segment, "Mach-O does not contain an executable entry point and code segment")
    require(libraries, "Executable is missing system dynamic-library dependencies")
    if target == "aarch64-apple-darwin":
        require(minimum >= MINIMUM_OS, "Apple Silicon deployment target must be macOS 11.0")
        require(signature is not None, "Apple Silicon executable has no code signature; link with -adhoc_codesign")
    signature_report = verify_adhoc_signature(data, *signature) if signature else {"kind": "unsigned"}
    return {"target": target, "minimum_macos": version_text(minimum), "declared_sdk": version_text(sdk), "system_libraries": libraries, "code_signature": signature_report, "sha256": hashlib.sha256(data).hexdigest()}


def render_icon(root: Path, staging: Path, node: str, modules: Path | None) -> bytes:
    from PIL import Image

    icns = staging / "Frame.icns"
    with Image.open(root / "assets/Frameflow.ico") as icon:
        icon.convert("RGBA").resize((1024, 1024), Image.Resampling.LANCZOS).save(icns, format="ICNS")
    with Image.open(icns) as icon:
        require(icon.size == (1024, 1024), "ICNS is missing its largest representation")
        for size in icon.info["sizes"]:
            icon.icns.getimage(size).load()
    return icns.read_bytes()


def write_zip(folder: Path, destination: Path) -> None:
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path in [folder, *sorted(folder.rglob("*"))]:
            require(not path.is_symlink(), "Symbolic links are not expected in this package")
            name = path.relative_to(folder.parent).as_posix()
            is_directory = path.is_dir()
            executable = name.endswith("/Frame.app/Contents/MacOS/frameflow")
            mode = (stat.S_IFDIR | 0o755) if is_directory else (stat.S_IFREG | (0o755 if executable else 0o644))
            info = zipfile.ZipInfo(name + ("/" if is_directory else ""))
            info.create_system = 3
            info.external_attr = (mode << 16) | (0x10 if is_directory else 0)
            archive.writestr(info, b"" if is_directory else path.read_bytes(), compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--binary", type=Path, help="Defaults to target/<target>/release/frameflow")
    parser.add_argument("--node", default="node", help="Node.js executable")
    parser.add_argument("--node-modules", type=Path, help="Directory containing @resvg/resvg-js")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    try:
        version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
        binary = args.binary or root / "target" / args.target / "release/frameflow"
        binary_data = binary.read_bytes()
        report = inspect_macho(binary_data, args.target)
        staging_root = root / "dist/.staging"
        staging_root.mkdir(parents=True, exist_ok=True)
        staging = Path(tempfile.mkdtemp(prefix="macos-", dir=staging_root))
        package_name = f"Frame-{version}-macos-{TARGETS[args.target][1]}"
        package = staging / package_name
        contents = package / "Frame.app/Contents"
        (contents / "MacOS/tools").mkdir(parents=True)
        (contents / "Resources").mkdir()
        (contents / "MacOS/frameflow").write_bytes(binary_data)
        (contents / "Resources/Frame.icns").write_bytes(render_icon(root, staging, args.node, args.node_modules))
        bundle_version = version.split("-", 1)[0].split("+", 1)[0]
        plist = {
            "CFBundleName": "Frameflow", "CFBundleDisplayName": "帧流 Frameflow",
            "CFBundleIdentifier": "app.frameflow.desktop", "CFBundleExecutable": "frameflow",
            "CFBundleInfoDictionaryVersion": "6.0", "CFBundlePackageType": "APPL",
            "CFBundleIconFile": "Frame", "CFBundleShortVersionString": bundle_version,
            "CFBundleVersion": bundle_version, "CFBundleSupportedPlatforms": ["MacOSX"],
            "LSMinimumSystemVersion": "11.0", "LSApplicationCategoryType": "public.app-category.video",
            "NSHighResolutionCapable": True,
        }
        (contents / "Info.plist").write_bytes(plistlib.dumps(plist, fmt=plistlib.FMT_XML, sort_keys=False))
        (contents / "PkgInfo").write_bytes(b"APPL????")
        (contents / "MacOS/tools/README.txt").write_text("Install ffmpeg and ffprobe with Homebrew, or select their directory in Frameflow settings.\nFFmpeg binaries are not included. https://ffmpeg.org/download.html\n", encoding="utf-8")
        for name in ("README.md", "LICENSE", "CHANGELOG.md", "THIRD_PARTY_NOTICES.md"):
            shutil.copyfile(root / name, package / name)
        screenshot_dir = root / "docs/screenshots"
        if screenshot_dir.is_dir():
            destination = package / "docs/screenshots"
            destination.mkdir(parents=True)
            for screenshot in screenshot_dir.glob("*.png"):
                shutil.copyfile(screenshot, destination / screenshot.name)
        report["application_version"] = version
        report["verification"] = "Static Mach-O, architecture, deployment target, dependencies and any embedded code-page hashes; macOS runtime testing and notarization are not performed by this packager."
        (package / "BUILD_INFO.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        archive_path = root / "dist" / f"{package_name}.zip"
        write_zip(package, archive_path)
        with zipfile.ZipFile(archive_path) as archive:
            entry = archive.getinfo(f"{package_name}/Frame.app/Contents/MacOS/frameflow")
            require(entry.create_system == 3 and (entry.external_attr >> 16) & 0o777 == 0o755, "ZIP executable permissions were not preserved")
            require(archive.read(entry) == binary_data, "Packaged executable differs from prebuilt input")
            require(archive.testzip() is None, "ZIP CRC verification failed")
            require(plistlib.loads(archive.read(f"{package_name}/Frame.app/Contents/Info.plist")) == plist, "Packaged Info.plist is invalid")
        checksum = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        archive_path.with_suffix(".zip.sha256").write_text(f"{checksum}  {archive_path.name}\n", encoding="ascii")
        print(json.dumps({"package": str(archive_path), "sha256": checksum, "binary": report}, indent=2))
    except (OSError, ValueError, subprocess.CalledProcessError, ImportError) as error:
        parser.exit(1, f"Packaging failed: {error}\n")


if __name__ == "__main__":
    main()
