// Modified by AlanWanco for shiorico; derived from Apache-2.0 iori/Shiori.
use anyhow::{Context, bail};
use clap::Parser;
use clap_handler::handler;
use self_update::{cargo_crate_version, get_target};

const RELEASE_PREFIX: &str = "shiorico-v";
const VERSIONS_URL: &str =
    "https://raw.githubusercontent.com/AlanWanco/shiorico/master/.versions/shiorico";

#[derive(Parser, Clone, Default, Debug)]
#[clap(name = "update")]
/// Update the shiorico binary
pub struct UpdateCommand {
    /// Custom URL for the versions file (one published release tag per line)
    #[clap(long, default_value = VERSIONS_URL)]
    versions_url: String,

    /// Target version or full shiorico release tag
    #[clap(short = 'v', long)]
    version: Option<String>,

    /// Target platform
    #[clap(long)]
    target: Option<String>,

    #[clap(short = 'y', long = "yes")]
    skip_confirm: bool,
}

fn release_tag(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    let version = value
        .strip_prefix(RELEASE_PREFIX)
        .or_else(|| value.strip_prefix('v'))
        .unwrap_or(value);
    if version.is_empty()
        || !version.starts_with(|c: char| c.is_ascii_digit())
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    {
        bail!("Invalid shiorico version or release tag: {value:?}");
    }
    Ok(format!("{RELEASE_PREFIX}{version}"))
}

fn latest_release_tag(index: &str) -> anyhow::Result<String> {
    let value = index
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .next_back()
        .context("No published shiorico release is listed in the versions file")?;
    release_tag(value)
}

fn release_asset(target: &str, variant: &str) -> (String, String) {
    let stem = format!("shiorico-{variant}-{target}");
    let extension = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    (
        format!("{stem}.{extension}"),
        format!("{stem}/{{{{ bin }}}}"),
    )
}

#[handler(UpdateCommand)]
pub async fn update_command(me: UpdateCommand) -> anyhow::Result<()> {
    let target = me.target.unwrap_or_else(|| get_target().to_string());
    let variant = if cfg!(feature = "ffmpeg") {
        "ffmpeg"
    } else {
        "core"
    };
    let (asset_name, archive_path) = release_asset(&target, variant);
    let target_version_tag = if let Some(version) = me.version {
        release_tag(&version)?
    } else {
        let index = reqwest::get(me.versions_url)
            .await?
            .error_for_status()?
            .text()
            .await?;
        latest_release_tag(&index)?
    };
    // self_update uses blocking HTTP; keep it outside Tokio's async runtime.
    let status = tokio::task::spawn_blocking(move || {
        self_update::backends::github::Update::configure()
            .repo_owner("AlanWanco")
            .repo_name("shiorico")
            .bin_name("shiorico")
            .identifier(&asset_name)
            .bin_path_in_archive(&archive_path)
            .target(&target)
            .target_version_tag(&target_version_tag)
            .show_download_progress(true)
            .current_version(cargo_crate_version!())
            .no_confirm(me.skip_confirm)
            .build()?
            .update()
    })
    .await??;

    println!("Update status: `{}`!", status.updated());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{latest_release_tag, release_asset, release_tag};
    use self_update::update::{Release, ReleaseAsset};

    #[test]
    fn accepts_version_or_full_tag() {
        for input in ["0.4.0", "v0.4.0", "shiorico-v0.4.0\n"] {
            assert_eq!(release_tag(input).unwrap(), "shiorico-v0.4.0");
        }
    }

    #[test]
    fn selects_last_published_index_entry() {
        let index = "# Published releases\nshiorico-v0.4.0\n\nshiorico-v0.4.1\n";
        assert_eq!(latest_release_tag(index).unwrap(), "shiorico-v0.4.1");
    }

    #[test]
    fn rejects_missing_or_invalid_index() {
        assert!(latest_release_tag("# No releases yet\n").is_err());
        assert!(latest_release_tag("<html>Not found</html>").is_err());
        assert!(release_tag("../shiori").is_err());
    }

    #[test]
    fn selects_binary_archive_instead_of_build_information() {
        for target in ["aarch64-apple-darwin", "x86_64-pc-windows-gnu"] {
            for variant in ["core", "ffmpeg"] {
                let (name, path) = release_asset(target, variant);
                let release = Release {
                    assets: [
                        format!("shiorico-{variant}-{target}-build-info.tar.gz"),
                        format!("shiorico-other-{target}.tar.gz"),
                        name.clone(),
                    ]
                    .into_iter()
                    .map(|name| ReleaseAsset {
                        name,
                        download_url: String::new(),
                    })
                    .collect(),
                    ..Default::default()
                };
                assert_eq!(release.asset_for(target, Some(&name)).unwrap().name, name);
                assert_eq!(path, format!("shiorico-{variant}-{target}/{{{{ bin }}}}"));
            }
        }
    }
}
