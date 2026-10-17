#!/usr/bin/env python3
"""Validate Cargo's release version and emit safe GitHub Actions outputs."""

import argparse
import re
import tomllib
from pathlib import Path


def metadata(manifest: Path, tag: str | None = None) -> dict[str, str]:
    version = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]["version"]
    match = re.fullmatch(
        r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
        r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
        r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?",
        version,
    )
    if match is None:
        raise ValueError(f"Unsupported release version: {version!r}")
    major, minor, patch, prerelease, _ = match.groups()
    if any(part.isdigit() and len(part) > 1 and part.startswith("0")
           for part in (prerelease or "").split(".")):
        raise ValueError("Numeric prerelease identifiers cannot have leading zeroes")
    if int(major) > 255 or int(minor) > 255 or int(patch) > 65535:
        raise ValueError("Release version exceeds Windows Installer version limits")
    expected_tag = f"release/v{version}"
    if tag is not None and tag != expected_tag:
        raise ValueError(f"Tag {tag!r} does not match Cargo version {version!r}")
    return {
        "VERSION": version,
        "VERSION_CLEAN": f"{major}.{minor}.{patch}",
        "DEBIAN_VERSION": version.replace("-", "~", 1) if prerelease else version,
        "RPM_RELEASE": f"0.{prerelease.replace('-', '.')}" if prerelease else "1",
        "TAG": expected_tag,
        "IS_PRERELEASE": str(prerelease is not None).lower(),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path("Cargo.toml"))
    parser.add_argument("--tag")
    args = parser.parse_args()
    try:
        values = metadata(args.manifest, args.tag)
    except (KeyError, TypeError, ValueError, OSError) as error:
        parser.exit(1, f"Release metadata error: {error}\n")
    for key, value in values.items():
        print(f"{key}={value}")


if __name__ == "__main__":
    main()
