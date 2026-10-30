#!/usr/bin/env python3
"""Build shiorico binary/source archives with licenses and pinned dependency source."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ("README.md", "LICENSE", "COPYING", "NOTICE", "LICENSING.md", "THIRD-PARTY-NOTICES.md")


def run(*args: str, cwd: Path = ROOT, **kwargs: object) -> subprocess.CompletedProcess:
    return subprocess.run(args, cwd=cwd, check=True, text=True, **kwargs)


def capture(*args: str, cwd: Path = ROOT) -> str:
    return run(*args, cwd=cwd, stdout=subprocess.PIPE).stdout.strip()


def revision() -> str:
    if (ROOT / "SOURCE_REVISION").exists():
        return (ROOT / "SOURCE_REVISION").read_text().strip()
    result = capture("git", "rev-parse", "HEAD")
    if capture("git", "status", "--porcelain"):
        result += "-dirty"
    return result


def validate_release_build() -> None:
    value = os.environ.get("SHIORICO_LOCAL_EPLUS")
    if value not in (None, "0", "1"):
        raise ValueError("SHIORICO_LOCAL_EPLUS must be 0 or 1")
    local = value == "1" or (value is None and (ROOT / ".shiorico-local-eplus").is_file())
    if local:
        raise RuntimeError("Release packaging requires a standard build")


def archive_tree(source: Path, output: Path) -> None:
    if output.suffix == ".zip":
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(source.rglob("*")):
                if path.is_file():
                    archive.write(path, path.relative_to(source.parent))
    else:
        with tarfile.open(output, "w:gz") as archive:
            archive.add(source, arcname=source.name)


def checksum(paths: list[Path], output: Path) -> None:
    lines = []
    for path in sorted(paths):
        digest = hashlib.file_digest(path.open("rb"), "sha256").hexdigest()
        lines.append(f"{digest}  {path.name}\n")
    output.write_text("".join(lines))


def generate_notices(target: str, variant: str, output: Path) -> None:
    command = [
        "cargo-about",
        "generate",
        "--format",
        "json",
        "--locked",
        "--fail",
        "--offline",
        "--manifest-path",
        "bin/shiori/Cargo.toml",
        "--config",
        "about.toml",
        "--target",
        target,
    ]
    if variant == "ffmpeg":
        command.extend(["--features", "ffmpeg"])
    report = json.loads(capture(*command))
    lines = ["shiorico Rust dependency license notices", f"Target: {target}; variant: {variant}", ""]
    for license_group in report["licenses"]:
        crates = sorted({f"{item['crate']['name']} {item['crate']['version']}" for item in license_group["used_by"]})
        lines.extend(["=" * 78, license_group["name"], "Used by: " + ", ".join(crates), "", license_group["text"], ""])
    output.write_text("\n".join(lines))


def copy_project(destination: Path) -> None:
    # Tracked source plus intentional, untracked migration files; ignore caches/artifacts.
    if (ROOT / ".git").exists():
        listing = run("git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", stdout=subprocess.PIPE)
        paths = [Path(name) for name in listing.stdout.split("\0") if name]
    else:
        paths = [path.relative_to(ROOT) for path in ROOT.rglob("*") if path.is_file() and "target" not in path.parts]
    for relative in paths:
        source = ROOT / relative
        if not source.is_file():
            continue
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)


def make_source(args: argparse.Namespace) -> Path:
    name = f"shiorico-source-{args.version}"
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="shiorico-source-", dir=args.output.parent) as temporary:
        project = Path(temporary) / name
        project.mkdir()
        copy_project(project)
        (project / "SOURCE_REVISION").write_text(revision() + "\n")
        vendor = args.vendor.resolve() if args.vendor else project / "vendor"
        config = capture(
            "cargo", "vendor", "--locked", "--offline", "--respect-source-config", "--versioned-dirs", str(vendor)
        )
        if args.vendor:
            # Avoid another full source copy while archiving an already generated vendor tree.
            (project / "vendor").symlink_to(vendor, target_is_directory=True)
        config = config.replace(str(vendor), "vendor")
        cargo_config = project / ".cargo/config.toml"
        cargo_config.parent.mkdir(exist_ok=True)
        with cargo_config.open("a") as stream:
            stream.write("\n# Release source includes all Cargo dependencies and bundled Bento4.\n" + config + "\n")
        bento4 = list((project / "vendor").glob("bento4-src-*/Bento4/Source/C++/Core/Ap4.h"))
        if not bento4:
            raise RuntimeError("Vendored source is missing the complete Bento4 C++ source")
        if args.ffmpeg_source:
            native = project / "native"
            native.mkdir()
            source = args.ffmpeg_source.resolve()
            expected = (ROOT / ".versions/ffmpeg").read_text().strip()
            if capture("git", "rev-parse", "HEAD", cwd=source) != expected:
                raise RuntimeError("FFmpeg source does not match .versions/ffmpeg")
            with tempfile.TemporaryFile() as snapshot:
                subprocess.run(["git", "archive", "HEAD"], cwd=source, stdout=snapshot, check=True)
                snapshot.seek(0)
                ffmpeg = native / "ffmpeg"
                ffmpeg.mkdir()
                with tarfile.open(fileobj=snapshot) as archive:
                    archive.extractall(ffmpeg, filter="data")
            (native / "FFMPEG_REVISION").write_text(expected + "\n")
        (project / "SOURCE-README.txt").write_text(
            "Corresponding source for shiorico " + args.version + "\n"
            "See docs/RELEASING.md for native prerequisites and exact build commands.\n"
            "Cargo dependencies and Bento4 are included in vendor/.\n"
            "The .cargo/config.toml source replacement supports cargo build --offline --locked.\n"
            "FFmpeg editions also require native/ffmpeg and the per-target build-info archive.\n"
            "SOURCE_REVISION identifies the application source; toolchain versions are in build-info.json.\n"
        )
        output = args.output / f"{name}.tar.gz"
        with tarfile.open(output, "w:gz", dereference=True) as archive:
            archive.add(project, arcname=name)
    checksum([output], args.output / "SHA256SUMS-source")
    print(output)
    return output


def make_binary(args: argparse.Namespace) -> Path:
    validate_release_build()
    executable = "shiorico.exe" if "windows" in args.target else "shiorico"
    binary = args.binary or ROOT / "target" / args.target / "release" / executable
    if not binary.is_file():
        raise RuntimeError(f"Missing binary: {binary}")
    args.output.mkdir(parents=True, exist_ok=True)
    name = f"shiorico-{args.variant}-{args.target}"
    source_name = f"shiorico-source-{args.version}.tar.gz"
    tag = f"shiorico-v{args.version}"
    source_url = f"https://github.com/AlanWanco/shiorico/releases/download/{tag}/{source_name}"
    with tempfile.TemporaryDirectory(prefix="shiorico-binary-", dir=args.output.parent) as temporary:
        package = Path(temporary) / name
        package.mkdir()
        shutil.copy2(binary, package / executable)
        for document in DOCS:
            shutil.copy2(ROOT / document, package / document)
        shutil.copytree(ROOT / "LICENSES", package / "LICENSES")
        generate_notices(args.target, args.variant, package / "THIRD-PARTY-LICENSES.txt")
        (package / "SOURCE.txt").write_text(f"Corresponding source: {source_url}\nSource revision: {revision()}\n")
        metadata = {
            "name": "shiorico",
            "version": args.version,
            "variant": args.variant,
            "target": args.target,
            "source_revision": revision(),
            "source_archive": source_name,
            "source_url": source_url,
            "rustc": capture("rustc", "-Vv"),
            "cargo": capture("cargo", "-V"),
            "features": ["ffmpeg"] if args.variant == "ffmpeg" else [],
            "ffmpeg_revision": (ROOT / ".versions/ffmpeg").read_text().strip() if args.variant == "ffmpeg" else None,
        }
        (package / "build-info.json").write_text(json.dumps(metadata, indent=2) + "\n")
        if args.variant == "ffmpeg":
            if not args.ffmpeg_info:
                raise RuntimeError("FFmpeg packages require --ffmpeg-info with the actual configure files")
            shutil.copytree(args.ffmpeg_info, package / "ffmpeg-build-info")
        output = args.output / (name + (".zip" if "windows" in args.target else ".tar.gz"))
        archive_tree(package, output)
        if args.variant == "ffmpeg":
            archive_tree(package / "ffmpeg-build-info", args.output / f"{name}-build-info.tar.gz")
    files = [output]
    if args.variant == "ffmpeg":
        files.append(args.output / f"{name}-build-info.tar.gz")
    checksum(files, args.output / f"SHA256SUMS-{args.variant}-{args.target}")
    print(output)
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=["source", "binary"])
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "dist/release")
    parser.add_argument("--target")
    parser.add_argument("--variant", choices=["core", "ffmpeg"], default="core")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--vendor", type=Path)
    parser.add_argument("--ffmpeg-source", type=Path)
    parser.add_argument("--ffmpeg-info", type=Path)
    args = parser.parse_args()
    if args.kind == "binary":
        if not args.target:
            parser.error("binary requires --target")
        make_binary(args)
    else:
        make_source(args)


if __name__ == "__main__":
    main()
