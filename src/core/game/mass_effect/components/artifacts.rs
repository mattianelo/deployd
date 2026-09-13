use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

use super::super::journal::{Identity, files};
use super::super::operation::Control;
use super::catalog::{Artifact, Component};

pub(super) type Progress = Arc<dyn Fn(usize, usize) + Send + Sync>;

pub(super) fn verify(bytes: &[u8], size: u64, hash: &str) -> Result<()> {
    ensure!(
        bytes.len() as u64 == size && format!("{:x}", Sha256::digest(bytes)) == hash,
        "MELE runtime component verification failed; preserve the file and obtain the pinned version again"
    );
    Ok(())
}

pub(super) async fn load(
    spec: Artifact,
    data: PathBuf,
    control: Control,
    progress: Progress,
) -> Result<Vec<u8>> {
    let path = data
        .join("mele-components")
        .join("artifacts")
        .join(spec.sha256);
    let cached = {
        let path = path.clone();
        let control = control.clone();
        tokio::task::spawn_blocking(move || -> Result<_> {
            control.check()?;
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    let parent = path.parent().context("Missing component cache parent")?;
                    files::verify(
                        parent,
                        spec.sha256,
                        Some(&Identity {
                            size: spec.size,
                            sha256: spec.sha256.into(),
                        }),
                        &control,
                    )?;
                    let file = std::fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                        .open(path)?;
                    let mut bytes = Vec::new();
                    file.take(spec.size + 1).read_to_end(&mut bytes)?;
                    verify(&bytes, spec.size, spec.sha256)?;
                    Ok(Some(bytes))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error).context("Cannot access MELE runtime cache"),
            }
        })
        .await
        .context("MELE runtime cache worker failed")??
    };
    if let Some(bytes) = cached {
        control.check()?;
        return Ok(bytes);
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    let mut response = tokio::select! {
        biased;
        _ = control.stopped() => anyhow::bail!("MELE runtime download cancelled"),
        response = client.get(spec.url).send() => response.context("Cannot download MELE runtime component; reconnect or reuse its verified cache")?.error_for_status()?,
    };
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size == spec.size),
        "MELE runtime download has an unexpected size"
    );
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = control.stopped() => anyhow::bail!("MELE runtime download cancelled"),
            chunk = response.chunk() => chunk.context("MELE runtime download interrupted")?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        ensure!(
            bytes.len() as u64 + chunk.len() as u64 <= spec.size,
            "MELE runtime download exceeds its pinned size"
        );
        bytes.extend_from_slice(&chunk);
        progress(bytes.len(), spec.size as usize);
    }
    tokio::task::spawn_blocking(move || {
        control.check()?;
        verify(&bytes, spec.size, spec.sha256)?;
        let parent = path.parent().context("Missing component cache parent")?;
        files::create_directory(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o400))?;
        temporary.as_file().sync_all()?;
        control.check()?;
        temporary
            .persist_noclobber(&path)
            .context("Runtime cache changed while downloading; preserve it and retry")?;
        files::sync(parent)?;
        Ok(bytes)
    })
    .await
    .context("MELE runtime cache publication failed")?
}

pub(super) fn payloads(
    component: Component,
    bytes: &[u8],
    control: &Control,
) -> Result<Vec<Vec<u8>>> {
    let artifact = component.artifact();
    verify(bytes, artifact.size, artifact.sha256)?;
    if component != Component::VisualCpp {
        return Ok(vec![bytes.to_vec()]);
    }
    let cabinet = bytes
        .get(630000..18721661)
        .context("Invalid pinned Visual C++ container")?;
    let inner = extract(cabinet, "a4", 1065893, control)?;
    verify(
        &inner,
        1065893,
        "6e8ee73933678c55973e83366e48e948b394c7a36743a36b7224c95de979b13f",
    )?;
    component
        .payloads()
        .iter()
        .map(|file| {
            let data = extract(&inner, &format!("{}_amd64", file.path), file.size, control)?;
            verify(&data, file.size, file.sha256)?;
            Ok(data)
        })
        .collect()
}

#[cfg(feature = "libarchive-fallback")]
fn extract(bytes: &[u8], member: &str, limit: u64, control: &Control) -> Result<Vec<u8>> {
    struct Bounded<'a> {
        bytes: Vec<u8>,
        limit: u64,
        control: &'a Control,
    }
    impl Write for Bounded<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.control.check().map_err(std::io::Error::other)?;
            if self.bytes.len() as u64 + bytes.len() as u64 > self.limit {
                return Err(std::io::Error::other(
                    "Runtime archive member exceeds its pinned size",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    control.check()?;
    let mut output = Bounded {
        bytes: Vec::new(),
        limit,
        control,
    };
    compress_tools::uncompress_archive_file(&mut std::io::Cursor::new(bytes), &mut output, member)
        .context("Cannot extract the pinned Visual C++ runtime payload")?;
    control.check()?;
    Ok(output.bytes)
}

#[cfg(not(feature = "libarchive-fallback"))]
fn extract(_: &[u8], _: &str, _: u64, _: &Control) -> Result<Vec<u8>> {
    anyhow::bail!(
        "This Deployd build lacks the archive backend required for app-local Visual C++ runtimes"
    )
}

pub(super) fn write(root: &Path, relative: &str, bytes: &[u8], control: &Control) -> Result<()> {
    control.check()?;
    let path = root.join(relative);
    files::create_directory(path.parent().context("Missing component output parent")?)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    control.check()
}
