use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DownloadSort {
    #[default]
    Default,
    Name,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DownloadFilter {
    #[default]
    All,
    Active,
    Completed,
}

/// Nexus Mods identity triple for a download: mod ID, file ID, and game domain.
#[derive(Debug, Clone, PartialEq)]
pub struct NexusIds {
    pub mod_id: i64,
    pub file_id: i64,
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadStatus {
    Downloading,
    Paused,
    Downloaded,
    Extracting,
    Installed,
    Failed,
}

impl DownloadStatus {
    pub fn as_db_str(&self) -> &str {
        match self {
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Downloaded => "downloaded",
            Self::Extracting => "extracting",
            Self::Installed => "installed",
            Self::Failed => "failed",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "paused" => Self::Paused,
            "installed" => Self::Installed,
            "failed" => Self::Failed,
            _ => Self::Downloaded,
        }
    }

    pub fn default_status_msg(&self) -> &str {
        match self {
            Self::Downloading => "Downloading...",
            Self::Paused => "Paused",
            Self::Downloaded => "Ready to install",
            Self::Extracting => "Extracting...",
            Self::Installed => "Installed",
            Self::Failed => "Install failed",
        }
    }

    pub fn restored_after_cancelled_install(was_reinstall: bool) -> Self {
        if was_reinstall {
            Self::Installed
        } else {
            Self::Downloaded
        }
    }

    pub fn restored_after_metadata_fetch(previous: &Self) -> Self {
        match previous {
            Self::Downloading | Self::Extracting => Self::Downloaded,
            stable => stable.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadEntry {
    pub id: String,
    pub mod_name: String,
    pub status: DownloadStatus,
    pub progress: f64,
    pub status_msg: String,
    pub error_msg: Option<String>,
    pub nexus_ids: Option<NexusIds>,
    pub archive_path: Option<PathBuf>,
    pub metadata_fetched: bool,
    /// Game domain for filtering (e.g., "skyrimspecialedition"). None = show for all games.
    pub game_domain: Option<String>,
    /// Nexus file name (e.g., "Main File" or "Update v5.2.1") for disambiguation.
    pub nexus_file_name: Option<String>,
    /// Whether this file is the primary/main file on Nexus.
    pub nexus_is_primary: bool,
    /// SHA-256 hex digest of the archive, set after a successful install.
    /// Used as a tiebreaker when nexus_file_id == 0 (disk-scanned, file ID unknown)
    /// so we can reset exactly the right download entry on mod removal.
    pub archive_hash: Option<String>,
    /// MD5 hex digest of the archive, computed lazily on first metadata fetch.
    /// Used for Nexus `md5_search` which returns the exact file entry without
    /// any filename parsing, bypassing CDN-timestamp ambiguity entirely.
    pub archive_md5: Option<String>,
    /// Version string resolved from Nexus (file version, or mod version as fallback).
    pub version: Option<String>,
    /// Mod author resolved from Nexus.
    pub author: Option<String>,
    /// Whether this entry is hidden from the downloads list (not deleted from disk).
    pub hidden: bool,
}

impl DownloadEntry {
    pub fn new(id: String, mod_name: String, nexus_ids: Option<NexusIds>) -> Self {
        let game_domain = nexus_ids.as_ref().map(|n| n.domain.clone());
        Self {
            id,
            mod_name,
            status: DownloadStatus::Downloading,
            progress: 0.0,
            status_msg: "Starting download...".to_string(),
            error_msg: None,
            nexus_ids,
            archive_path: None,
            metadata_fetched: false,
            game_domain,
            nexus_file_name: None,
            nexus_is_primary: false,
            archive_hash: None,
            archive_md5: None,
            version: None,
            author: None,
            hidden: false,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            DownloadStatus::Downloading | DownloadStatus::Extracting
        )
    }

    pub fn is_installable(&self) -> bool {
        matches!(
            self.status,
            DownloadStatus::Downloaded | DownloadStatus::Failed
        ) && self.archive_path.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::DownloadStatus;

    #[test]
    fn cancelled_fresh_install_returns_to_downloaded() {
        assert_eq!(
            DownloadStatus::restored_after_cancelled_install(false),
            DownloadStatus::Downloaded
        );
    }

    #[test]
    fn cancelled_reinstall_returns_to_installed() {
        assert_eq!(
            DownloadStatus::restored_after_cancelled_install(true),
            DownloadStatus::Installed
        );
    }

    #[test]
    fn metadata_fetch_restores_stable_status() {
        assert_eq!(
            DownloadStatus::restored_after_metadata_fetch(&DownloadStatus::Installed),
            DownloadStatus::Installed
        );
        assert_eq!(
            DownloadStatus::restored_after_metadata_fetch(&DownloadStatus::Failed),
            DownloadStatus::Failed
        );
    }

    #[test]
    fn metadata_fetch_busy_status_falls_back_to_downloaded() {
        assert_eq!(
            DownloadStatus::restored_after_metadata_fetch(&DownloadStatus::Downloading),
            DownloadStatus::Downloaded
        );
        assert_eq!(
            DownloadStatus::restored_after_metadata_fetch(&DownloadStatus::Extracting),
            DownloadStatus::Downloaded
        );
    }
}
