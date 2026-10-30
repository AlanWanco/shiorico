#!/usr/bin/env python3
"""Validate complete, matching shiorico release materials before publication."""

from __future__ import annotations

import argparse
import hashlib
import json
import tarfile
import zipfile
from pathlib import Path

TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-gnu",
)
REQUIRED = {
    "README.md",
    "LICENSE",
    "COPYING",
    "NOTICE",
    "LICENSING.md",
    "THIRD-PARTY-NOTICES.md",
    "THIRD-PARTY-LICENSES.txt",
    "SOURCE.txt",
    "build-info.json",
    "LICENSES/Bento4.txt",
}


def inspect_archive(path: Path) -> tuple[set[str], dict[str, bytes]]:
    if path.suffix == ".zip":
        with zipfile.ZipFile(path) as archive:
            names = archive.namelist()
            data = {name: archive.read(name) for name in names if name.endswith(("build-info.json", "SOURCE_REVISION"))}
    else:
        with tarfile.open(path) as archive:
            names = archive.getnames()
            data = {}
            for member in archive:
                if member.isfile() and member.name.endswith(("build-info.json", "SOURCE_REVISION")):
                    stream = archive.extractfile(member)
                    assert stream is not None
                    data[member.name] = stream.read()
    return set(names), data


def validate(directory: Path, version: str, variants: list[str], targets: list[str]) -> None:
    source_name = f"shiorico-source-{version}"
    source = directory / f"{source_name}.tar.gz"
    source_files, source_data = inspect_archive(source)
    for required in ["Cargo.lock", "COPYING", "LICENSE", "NOTICE", "scripts/package-release.py", ".cargo/config.toml"]:
        assert f"{source_name}/{required}" in source_files, f"Missing source material: {required}"
    assert any(
        "/vendor/bento4-src-" in name and name.endswith("/Bento4/Source/C++/Core/Ap4.h") for name in source_files
    ), "Bento4 source missing"
    assert f"{source_name}/.shiorico-local-eplus" not in source_files, "Local Eplus marker in release source"
    source_revision = source_data[f"{source_name}/SOURCE_REVISION"].decode().strip()
    if "ffmpeg" in variants:
        assert f"{source_name}/native/ffmpeg/configure" in source_files, "FFmpeg source missing"
        assert f"{source_name}/native/ffmpeg/COPYING.LGPLv2.1" in source_files, "FFmpeg license missing"
    assets = [source]
    for variant in variants:
        for target in targets:
            stem = f"shiorico-{variant}-{target}"
            archive = directory / (stem + (".zip" if "windows" in target else ".tar.gz"))
            names, data = inspect_archive(archive)
            required = REQUIRED | {"shiorico.exe" if "windows" in target else "shiorico"}
            if variant == "ffmpeg":
                required |= {
                    "ffmpeg-build-info/configure-command.txt",
                    "ffmpeg-build-info/config.h",
                    "ffmpeg-build-info/config.mak",
                    "ffmpeg-build-info/SOURCE_REVISION",
                    "ffmpeg-build-info/licenses/LICENSE.md",
                    "ffmpeg-build-info/licenses/COPYING.LGPLv2.1",
                }
            assert {f"{stem}/{name}" for name in required} <= names, f"Missing binary materials: {archive.name}"
            info = json.loads(data[f"{stem}/build-info.json"])
            assert (info["version"], info["variant"], info["target"], info["source_revision"]) == (
                version,
                variant,
                target,
                source_revision,
            ), f"Source/binary mismatch: {archive.name}"
            assert info["eplus_preparing_policy"] == "stop", f"Local Eplus policy in release: {archive.name}"
            assert info["source_archive"] == source.name, f"Incorrect source link: {archive.name}"
            assets.append(archive)
            if variant == "ffmpeg":
                native_revision = data[f"{stem}/ffmpeg-build-info/SOURCE_REVISION"].decode().strip()
                assert info["ffmpeg_revision"] == native_revision, f"FFmpeg revision mismatch: {archive.name}"
                build_info = directory / f"{stem}-build-info.tar.gz"
                build_names, _ = inspect_archive(build_info)
                assert "ffmpeg-build-info/configure-command.txt" in build_names
                assets.append(build_info)
    lines = []
    for asset in sorted(assets):
        with asset.open("rb") as stream:
            lines.append(f"{hashlib.file_digest(stream, 'sha256').hexdigest()}  {asset.name}\n")
    assert not source_revision.endswith("-dirty"), "Release source contains uncommitted changes"
    # Each producer's checksums must agree before creating the release manifest.
    expected = {line.split()[1]: line.split()[0] for line in lines}
    for partial in directory.glob("SHA256SUMS-*"):
        for line in partial.read_text().splitlines():
            digest, name = line.split()
            assert expected.get(name) == digest, f"Checksum mismatch: {name}"
    (directory / "SHA256SUMS").write_text("".join(lines))
    print(f"Validated {len(assets)} archives at source revision {source_revision}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--variants", nargs="+", choices=["core", "ffmpeg"], default=["core", "ffmpeg"])
    parser.add_argument("--targets", nargs="+", default=list(TARGETS))
    args = parser.parse_args()
    validate(args.directory, args.version, args.variants, args.targets)


if __name__ == "__main__":
    main()
