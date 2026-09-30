#!/usr/bin/env python3
"""Regenerate bundled Rust notices from saved cargo metadata --locked JSON."""

import json
import sys
from pathlib import Path


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    metadata = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8-sig"))
    destination = root / "THIRD_PARTY_NOTICES.md"
    heading = destination.read_text(encoding="utf-8").split("| Package |", 1)[0]
    packages = sorted(
        (p for p in metadata["packages"] if p["source"] is not None),
        key=lambda p: (p["name"].lower(), p["version"]),
    )
    lines = [heading.rstrip(), "", "| Package | Version | Declared license |", "| --- | --- | --- |"]
    for package in packages:
        license_name = package.get("license") or "See package license file"
        lines.append(f'| {package["name"]} | {package["version"]} | {license_name} |')
    count = 0
    for package in packages:
        folder = Path(package["manifest_path"]).parent
        candidates = {
            path
            for path in folder.iterdir()
            if path.is_file()
            and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"))
        }
        if package.get("license_file"):
            license_path = folder / package["license_file"]
            if license_path.is_file():
                candidates.add(license_path)
        if not candidates:
            continue
        lines.extend(["", f'## {package["name"]} {package["version"]}'])
        for path in sorted(candidates):
            text = path.read_text(encoding="utf-8", errors="replace").rstrip()
            fence = "`" * max(4, max((len(s) for s in text.split() if set(s) == {"`"}), default=0) + 1)
            lines.extend(["", f"### {path.name}", "", fence + "text", text, fence])
            count += 1
    destination.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({"packages": len(packages), "notice_files": count, "output": str(destination)}))


if __name__ == "__main__":
    main()
