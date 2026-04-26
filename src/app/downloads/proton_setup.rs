use anyhow::{Context, Result};
use std::path::PathBuf;

const RELEASES_URL: &str =
    "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases/latest";

/// Download and extract the latest Proton GE release into the Steam
/// compatibility-tools directory searched by `find_proton_runtime()`.
pub async fn download_proton_ge() -> Result<()> {
    let dest_dir = proton_install_dir()?;
    std::fs::create_dir_all(&dest_dir).context("create compatibilitytools.d")?;

    let client = reqwest::Client::builder()
        .user_agent(concat!("deployd/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build reqwest client")?;

    let api_response = client
        .get(RELEASES_URL)
        .send()
        .await
        .context("fetch GitHub release")?;

    if !api_response.status().is_success() {
        let status = api_response.status();
        let body = api_response
            .text()
            .await
            .unwrap_or_else(|_| String::from("<unreadable>"));
        anyhow::bail!("GitHub API error {status}: {body}");
    }

    let release: serde_json::Value = api_response
        .json()
        .await
        .context("parse release JSON")?;

    let (asset_url, asset_name) = release["assets"]
        .as_array()
        .and_then(|assets| {
            assets.iter().find(|a| {
                let name = a["name"].as_str().unwrap_or("");
                name.ends_with(".tar.gz") && !name.contains("sha512")
            })
        })
        .and_then(|a| {
            Some((
                a["browser_download_url"].as_str()?.to_owned(),
                a["name"].as_str()?.to_owned(),
            ))
        })
        .context("no .tar.gz asset in latest Proton GE release")?;

    let tmp = tempfile::NamedTempFile::new_in(&dest_dir).context("create temp file")?;
    let tmp_path = tmp.path().to_owned();

    let mut response = client
        .get(&asset_url)
        .send()
        .await
        .context("download Proton GE")?
        .error_for_status()
        .context("download error")?;

    {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&tmp_path)
            .await
            .context("open temp file")?;
        while let Some(chunk) = response.chunk().await.context("read download chunk")? {
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                .await
                .context("write chunk")?;
        }
    }

    let status = tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tmp_path)
        .arg("-C")
        .arg(&dest_dir)
        .status()
        .await
        .context("run tar")?;

    anyhow::ensure!(
        status.success(),
        "tar extraction failed ({status}): {asset_name}"
    );

    Ok(())
}

fn proton_install_dir() -> Result<PathBuf> {
    // Snap: use SNAP_USER_COMMON so the runtime persists across snap revisions.
    if let Ok(snap_common) = std::env::var("SNAP_USER_COMMON") {
        return Ok(PathBuf::from(snap_common).join("Steam/compatibilitytools.d"));
    }
    let xdg = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .context("cannot determine XDG_DATA_HOME")?;
    Ok(xdg.join("Steam/compatibilitytools.d"))
}
