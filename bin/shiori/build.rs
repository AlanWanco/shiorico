// Modified by AlanWanco for shiorico; derived from Apache-2.0 iori/Shiori.
use std::env;
use std::path::PathBuf;
use std::process::Command;

fn get_commit_hash() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8(output.stdout).ok()?;
    let hash = hash.trim();
    (!hash.is_empty()).then(|| hash.to_owned())
}

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    let variant = if cfg!(feature = "ffmpeg") {
        "ffmpeg"
    } else {
        "core"
    };
    let source_revision = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../SOURCE_REVISION");
    let hash = env::var("SHIORICO_BUILD_COMMIT")
        .ok()
        .or_else(|| {
            std::fs::read_to_string(&source_revision)
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .or_else(get_commit_hash)
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SHIORICO_VERSION={version} ({variant}-{hash})");
    println!("cargo:rerun-if-env-changed=SHIORICO_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=windows.manifest.xml");

    // Rebuild the displayed revision after a checkout, including linked worktrees.
    for name in ["HEAD", "packed-refs"] {
        if let Ok(output) = Command::new("git")
            .args(["rev-parse", "--git-path", name])
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout);
                println!("cargo:rerun-if-changed={}", path.trim());
            }
        }
    }
    if let Ok(output) = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
    {
        if output.status.success() {
            let reference = String::from_utf8_lossy(&output.stdout);
            if let Ok(path) = Command::new("git")
                .args(["rev-parse", "--git-path", reference.trim()])
                .output()
            {
                if path.status.success() {
                    let path = String::from_utf8_lossy(&path.stdout);
                    println!("cargo:rerun-if-changed={}", path.trim());
                }
            }
        }
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_manifest_file("windows.manifest.xml");
        res.set("ProductName", "shiorico");
        res.set("FileDescription", "shiorico stream downloader");
        res.set("OriginalFilename", "shiorico.exe");
        res.compile().expect("Failed to compile Windows resources");
    }

    // Include the unpacked source revision when building without Git metadata.
    println!("cargo:rerun-if-changed={}", source_revision.display());
}
