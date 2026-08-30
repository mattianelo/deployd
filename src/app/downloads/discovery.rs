use std::path::{Path, PathBuf};

use crate::core::game;
use crate::core::nexus_identity::parse_nexus_mod_id;
use crate::models::download::NexusIds;

pub(super) struct DiscoveredArchive {
    pub path: PathBuf,
    pub mod_name: String,
    pub game_domain: Option<String>,
    pub nexus_ids: Option<NexusIds>,
}

pub(super) fn scan_archives(base_dir: &Path) -> Vec<DiscoveredArchive> {
    let mut archives = Vec::new();
    for domain in game::all_nexus_domains() {
        collect_archives(base_dir.join(domain), Some(domain), &mut archives);
    }
    collect_archives(base_dir.to_path_buf(), None, &mut archives);
    archives
}

fn collect_archives(
    directory: PathBuf,
    domain: Option<&str>,
    archives: &mut Vec<DiscoveredArchive>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !is_archive(&path) {
            continue;
        }
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let nexus_ids = parse_nexus_mod_id(&file_name).map(|mod_id| NexusIds {
            mod_id,
            file_id: 0,
            domain: domain.unwrap_or_default().to_string(),
        });
        let mod_name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        archives.push(DiscoveredArchive {
            path,
            mod_name,
            game_domain: domain.map(str::to_string),
            nexus_ids,
        });
    }
}

fn is_archive(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["zip", "7z", "rar", "dazip"].contains(&extension.to_lowercase().as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_game_folder_before_flat_downloads() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let domain = game::all_nexus_domains()
            .into_iter()
            .next()
            .expect("known games provide a Nexus domain");
        let game_dir = temp.path().join(domain);
        std::fs::create_dir(&game_dir)?;
        std::fs::write(game_dir.join("game-mod.zip"), b"archive")?;
        std::fs::write(temp.path().join("manual-mod.7z"), b"archive")?;

        let archives = scan_archives(temp.path());

        assert_eq!(archives.len(), 2);
        assert_eq!(archives[0].game_domain.as_deref(), Some(domain));
        assert_eq!(archives[1].game_domain, None);
        Ok(())
    }

    #[test]
    fn recognizes_supported_archive_extensions_case_insensitively() {
        for path in ["mod.zip", "mod.7z", "mod.RAR", "mod.DAZIP"] {
            assert!(is_archive(Path::new(path)), "{path}");
        }
        assert!(!is_archive(Path::new("mod.tar.gz")));
    }
}
