use std::fs;
use std::io::Read as _;
use std::path::Path;

use anyhow::{Context, Result};
use md5::Md5;
use sha2::{Digest, Sha256};

/// Compute a SHA-256 hex digest of an archive file.
///
/// Used to detect duplicate installations: if the same archive is chosen
/// twice, the hash matches and we can offer "already installed" feedback.
/// Reads the entire file sequentially; for large archives this is I/O-bound
/// but much cheaper than a full extraction.
pub fn hash_archive_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("Cannot open archive for hashing: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Compute an MD5 hex digest of an archive file.
///
/// Used for Nexus Mods' `md5_search` API endpoint which returns the exact
/// file entry (including `file_id`, name, and version) for a given archive,
/// bypassing any filename ambiguity introduced by CDN-appended timestamps.
pub fn compute_md5(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("Cannot open archive for MD5: {}", path.display()))?;
    let mut hasher = Md5::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
