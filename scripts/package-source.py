#!/usr/bin/env python3
"""Create the versioned source ZIP using an explicit repository allowlist."""

from __future__ import annotations

import hashlib
import json
import stat
import tempfile
import tomllib
import zipfile
from pathlib import Path


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    files = [root / name for name in (
        "Cargo.toml", "Cargo.lock", "build.rs", "README.md", "LICENSE", "CHANGELOG.md",
        "THIRD_PARTY_NOTICES.md", ".gitignore", "FFmpeg功能检查-0.2.1.md",
    )]
    for directory in ("src", "assets", "scripts", ".github", "docs"):
        for path in (root / directory).rglob("*"):
            if path.is_symlink():
                raise ValueError(f"Source package does not support symlinks: {path}")
            if path.is_file() and "__pycache__" not in path.parts and path.suffix not in (".pyc", ".pyo"):
                files.append(path)
    prefix = f"Frameflow-{version}-source"
    contents = {f"{prefix}/{path.relative_to(root).as_posix()}": path.read_bytes() for path in sorted(files)}
    staging_root = root / "dist/.staging"
    staging_root.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix="source-", dir=staging_root))
    archive_path = staging / f"{prefix}.zip"
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, data in contents.items():
            entry = zipfile.ZipInfo(name)
            entry.create_system = 3
            executable = name.endswith((".sh", ".py")) and f"{prefix}/scripts/" in name
            entry.external_attr = (stat.S_IFREG | (0o755 if executable else 0o644)) << 16
            archive.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
    with zipfile.ZipFile(archive_path) as archive:
        if archive.testzip() is not None or set(archive.namelist()) != set(contents):
            raise ValueError("Source ZIP failed its entry or CRC check")
        if any(archive.read(name) != data for name, data in contents.items()):
            raise ValueError("Source ZIP differs from the source snapshot")
    destination = root / "dist" / archive_path.name
    archive_path.replace(destination)
    staging.rmdir()
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    destination.with_suffix(".zip.sha256").write_text(f"{digest}  {destination.name}\n", encoding="ascii")
    print(json.dumps({"package": str(destination), "files": len(contents), "bytes": destination.stat().st_size, "sha256": digest}, indent=2))


if __name__ == "__main__":
    main()
