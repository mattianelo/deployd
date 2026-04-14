use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use reqwest::Client;
use tokio::io::AsyncWriteExt;

use crate::models::proton_release::ProtonRelease;
use crate::utils::paths;

const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases";

/// How many releases to fetch from the GitHub API in one call.
const RELEASES_TO_FETCH: u8 = 10;

/// Filename of the symlink inside `runtimes_dir()` that points to the active version.
const CURRENT_LINK: &str = "current";

// ── Public API ────────────────────────────────────────────────────────────────

/// Fetch the latest ProtonGE releases from GitHub and annotate each with whether
/// it is already installed locally.
///
/// Returns at most `RELEASES_TO_FETCH` entries, newest first.
/// Network errors are propagated; the caller should handle them gracefully.
pub async fn list_releases() -> Result<Vec<ProtonRelease>> {
    let client = build_client()?;

    let url = format!("{GITHUB_RELEASES_URL}?per_page={RELEASES_TO_FETCH}");
    let json: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .context("Failed to reach GitHub releases API")?
        .error_for_status()
        .context("GitHub API returned an error status")?
        .json()
        .await
        .context("Failed to parse GitHub releases JSON")?;

    let runtimes = paths::runtimes_dir().ok();

    let releases = json
        .as_array()
        .ok_or_else(|| anyhow!("Unexpected JSON shape from GitHub API"))?
        .iter()
        .filter_map(|entry| {
            let tag = entry["tag_name"].as_str()?.to_owned();
            // Pick the .tar.gz asset (skip .sha512sum etc.)
            let download_url = entry["assets"]
                .as_array()?
                .iter()
                .find(|a| {
                    a["name"]
                        .as_str()
                        .is_some_and(|n| n.ends_with(".tar.gz"))
                })?["browser_download_url"]
                .as_str()?
                .to_owned();

            let installed = runtimes
                .as_ref()
                .is_some_and(|r| r.join(&tag).join("files/bin/wine").exists());

            Some(ProtonRelease {
                tag,
                download_url,
                installed,
            })
        })
        .collect();

    Ok(releases)
}

/// Return the list of ProtonGE versions currently installed on disk, newest tag first.
pub fn installed_versions() -> Vec<String> {
    let Ok(dir) = paths::runtimes_dir() else {
        return vec![];
    };
    let mut versions: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_type().is_ok_and(|t| t.is_dir()) && e.path().join("files/bin/wine").exists()
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    versions.sort_by(|a, b| b.cmp(a)); // newest first (lexicographic works for GE-ProtonX-Y)
    versions
}

/// Returns the path to the active ProtonGE directory, if one is set.
///
/// Resolves the `runtimes/current` symlink.
pub fn active_runtime_path() -> Option<PathBuf> {
    let link = paths::runtimes_dir().ok()?.join(CURRENT_LINK);
    let target = std::fs::read_link(&link).ok()?;
    let resolved = if target.is_absolute() {
        target
    } else {
        paths::runtimes_dir().ok()?.join(target)
    };
    if resolved.is_dir() {
        Some(resolved)
    } else {
        None
    }
}

/// Returns the tag name of the active runtime (the name of the directory `current` points to).
pub fn active_runtime_tag() -> Option<String> {
    active_runtime_path()?
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Download and extract a ProtonGE release tarball into `runtimes/<tag>/`.
///
/// `progress` receives `(downloaded_bytes, total_bytes)` pairs periodically so
/// the caller can drive a progress bar. `total_bytes` is `0` when the server
/// does not report a Content-Length.
pub async fn install_release(
    tag: &str,
    download_url: &str,
    progress: tokio::sync::mpsc::Sender<(u64, u64)>,
) -> Result<()> {
    let runtimes = paths::runtimes_dir().context("Cannot determine runtimes directory")?;
    std::fs::create_dir_all(&runtimes).context("Failed to create runtimes directory")?;

    let dest_dir = runtimes.join(tag);
    if dest_dir.join("files/bin/wine").exists() {
        return Ok(()); // Already installed.
    }

    // Remove any partial extraction left by a previous failed attempt so tar
    // does not fail when it tries to create a directory that already exists.
    if dest_dir.exists() {
        std::fs::remove_dir_all(&dest_dir).with_context(|| {
            format!(
                "Failed to remove partial extraction at {}",
                dest_dir.display()
            )
        })?;
    }

    // Download to a temp file next to the runtimes dir so we can move atomically.
    let tmp_path = runtimes.join(format!("{tag}.tar.gz.tmp"));

    download_file(download_url, &tmp_path, progress)
        .await
        .context("Download failed")?;

    // Extract: `tar xzf <tmp> -C <runtimes>`
    // ProtonGE tarballs contain a single top-level directory named after the tag.
    let status = tokio::process::Command::new("tar")
        .arg("xzf")
        .arg(&tmp_path)
        .arg("-C")
        .arg(&runtimes)
        .status()
        .await
        .context("Failed to run tar")?;

    if !status.success() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(anyhow!("tar extraction failed for {tag}: {status}"));
    }

    let _ = std::fs::remove_file(&tmp_path); // clean up after successful extraction
    Ok(())
}

/// Remove an installed ProtonGE version from disk.
///
/// If the removed version was the active one, the `current` symlink is also deleted.
pub fn remove_release(tag: &str) -> Result<()> {
    let runtimes = paths::runtimes_dir().context("Cannot determine runtimes directory")?;
    let dir = runtimes.join(tag);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to remove {}", dir.display()))?;
    }

    // Clear the `current` symlink if it was pointing at this version.
    if active_runtime_tag().as_deref() == Some(tag) {
        let link = runtimes.join(CURRENT_LINK);
        let _ = std::fs::remove_file(&link);
    }

    Ok(())
}

/// Set the active ProtonGE runtime by updating the `runtimes/current` symlink.
///
/// `tag` must be an installed version (i.e. `runtimes/<tag>/files/bin/wine` must exist).
pub fn set_active_runtime(tag: &str) -> Result<()> {
    let runtimes = paths::runtimes_dir().context("Cannot determine runtimes directory")?;
    let target = runtimes.join(tag);

    if !target.join("files/bin/wine").exists() {
        return Err(anyhow!("ProtonGE version '{tag}' is not installed"));
    }

    let link = runtimes.join(CURRENT_LINK);

    // Remove stale symlink if any.
    if link.exists() || link.is_symlink() {
        std::fs::remove_file(&link).context("Failed to remove old 'current' symlink")?;
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link)
        .with_context(|| format!("Failed to create 'current' symlink → {}", target.display()))?;

    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn build_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("deployd/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to build HTTP client")
}

/// Download `url` to `dest`, streaming with progress reports.
async fn download_file(
    url: &str,
    dest: &std::path::Path,
    progress: tokio::sync::mpsc::Sender<(u64, u64)>,
) -> Result<()> {
    let client = build_client()?;
    let resp = client
        .get(url)
        .send()
        .await
        .context("GET request failed")?
        .error_for_status()
        .context("Server returned error status")?;

    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("Cannot create {}", dest.display()))?;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Stream error while downloading")?;
        file.write_all(&chunk)
            .await
            .context("Write error while downloading")?;
        downloaded += chunk.len() as u64;
        let _ = progress.try_send((downloaded, total));
    }

    file.flush().await.context("Flush error")?;
    Ok(())
}
