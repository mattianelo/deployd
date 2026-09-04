use std::collections::BTreeSet;

use anyhow::{Context, Result};
use tempfile::TempDir;

use super::application::{deploy_with_backup_dir, deployment_preflight};
use super::backup::files_match;
use super::filesystem::{find_existing_deploy_path_case_insensitive, resolve_deploy_path};
use super::purge::purge;
use super::report::VanillaReplacementStatus;
use crate::core::game::{self, KnownGameOption, known_game_options};
use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameEngine};
use crate::models::manifest::ModFile;
use crate::models::mod_entry::{InstallTarget, ModEntry};

#[derive(Clone, Copy, Debug)]
enum FinalAction {
    Deploy,
    Purge,
}

impl FinalAction {
    fn name(self) -> &'static str {
        match self {
            Self::Deploy => "Deploy",
            Self::Purge => "Purge",
        }
    }
}

struct LifecyclePath<'a> {
    original: &'a str,
    case_variant: &'a str,
    backup_anchor: &'a str,
}

fn mod_entry(id: &str, game_id: &str, enabled: bool, priority: i32) -> ModEntry {
    ModEntry {
        id: id.to_string(),
        game_id: game_id.to_string(),
        name: id.to_string(),
        archive_hash: None,
        archive_path: None,
        installed_at: None,
        enabled,
        priority,
        nexus_mod_id: None,
        nexus_file_id: None,
        nexus_domain: None,
        version: None,
        author: None,
        nexus_description: None,
        latest_version: None,
        nexus_file_name: None,
        nexus_is_primary: false,
        archive_md5: None,
        install_target: InstallTarget::Data,
        notes: None,
    }
}

fn configured_game(option: &KnownGameOption, temp: &TempDir) -> Result<Game> {
    let game_root = temp.path().join("games").join(option.deployd_id);
    let wine_prefix = if *option.engine == GameEngine::Eclipse {
        let prefix = temp.path().join("prefix");
        std::fs::create_dir_all(prefix.join("drive_c/users/test-user"))?;
        Some(prefix)
    } else {
        None
    };
    Ok(Game {
        id: option.deployd_id.to_string(),
        title: option.title.to_string(),
        path: game_root,
        data_subdir: option.data_subdir.to_string(),
        engine: option.engine.clone(),
        wine_prefix,
    })
}

async fn run_vanilla_lifecycle(
    option: &KnownGameOption,
    path: &LifecyclePath<'_>,
    final_action: FinalAction,
) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let game = configured_game(option, &temp)?;
    let game_data = game::deploy_dir(&game);
    let cache_root = temp.path().join("cache");
    let backup_dir = temp.path().join("vanilla-backups").join(&game.id);
    let expected_original = temp.path().join("expected-original.bin");
    let canonical_path = path.original.to_lowercase();
    let live_original = resolve_deploy_path(path.original, &game.path, &game_data)?;
    let cache_a = cache_root.join("mod-a/content.bin");
    let cache_b = cache_root.join("mod-b/content.bin");
    let vanilla_content = format!("vanilla:{}:{}", game.id, path.original).into_bytes();
    let mod_a_content = format!("mod-a:{}:{}", game.id, path.original).into_bytes();
    let mod_b_content = format!("mod-b:{}:{}", game.id, path.original).into_bytes();

    std::fs::create_dir_all(
        live_original
            .parent()
            .context("vanilla file has no parent directory")?,
    )?;
    std::fs::create_dir_all(cache_a.parent().context("mod A cache has no parent")?)?;
    std::fs::create_dir_all(cache_b.parent().context("mod B cache has no parent")?)?;
    std::fs::write(&live_original, &vanilla_content)?;
    std::fs::write(&expected_original, &vanilla_content)?;
    std::fs::write(&cache_a, &mod_a_content)?;
    std::fs::write(&cache_b, &mod_b_content)?;

    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    tracker
        .insert_mod(&mod_entry("mod-a", &game.id, true, 1))
        .await?;
    tracker
        .insert_mod(&mod_entry("mod-b", &game.id, false, 2))
        .await?;
    tracker
        .record_files(&[
            ModFile {
                mod_id: "mod-a".to_string(),
                game_rel_lowercase: canonical_path.clone(),
                game_rel_original: path.original.to_string(),
                cache_path: cache_a.to_string_lossy().into_owned(),
            },
            ModFile {
                mod_id: "mod-b".to_string(),
                game_rel_lowercase: canonical_path.clone(),
                game_rel_original: path.case_variant.to_string(),
                cache_path: cache_b.to_string_lossy().into_owned(),
            },
        ])
        .await?;
    tracker
        .reset_vanilla_snapshot(
            &game.id,
            &[(canonical_path.clone(), vanilla_content.len() as u64, 0)],
        )
        .await?;

    let ready = deployment_preflight(&game, &tracker).await?;
    assert!(
        ready.protect_vanilla_files,
        "{} must default vanilla protection to enabled",
        game.id
    );
    assert_eq!(
        ready.vanilla_replacements.len(),
        1,
        "{} must detect one vanilla replacement",
        game.id
    );
    assert_eq!(
        ready.vanilla_replacements[0].status,
        VanillaReplacementStatus::ReadyToBackUp,
        "{} must report its original as ready to back up",
        game.id
    );

    let deployed_a =
        deploy_with_backup_dir(&game, &tracker, &cache_root, true, Some(&backup_dir)).await?;
    assert_eq!(deployed_a.vanilla_files_backed_up, 1, "{}", game.id);
    assert_eq!(deployed_a.vanilla_files_restored, 0, "{}", game.id);
    assert_eq!(
        read_live_file(&game, path.original)?,
        mod_a_content,
        "{}",
        game.id
    );

    let original_record = tracker
        .get_vanilla_backup(&game.id, &canonical_path)
        .await?
        .with_context(|| format!("{} did not record its vanilla backup", game.id))?;
    assert_eq!(original_record.game_rel_path, path.original, "{}", game.id);
    assert!(
        original_record.backup_path.starts_with(&backup_dir),
        "{} stored its backup outside the injected per-game directory",
        game.id
    );
    assert!(
        original_record
            .backup_path
            .strip_prefix(&backup_dir)?
            .starts_with(path.backup_anchor),
        "{} stored '{}' under the wrong backup anchor",
        game.id,
        path.original
    );
    assert!(files_match(
        &expected_original,
        &original_record.backup_path
    )?);

    tracker.toggle_mod("mod-a", false).await?;
    tracker.toggle_mod("mod-b", true).await?;
    let protected = deployment_preflight(&game, &tracker).await?;
    assert_eq!(
        protected.vanilla_replacements.len(),
        1,
        "{} must warn again for a changed winner",
        game.id
    );
    assert_eq!(
        protected.vanilla_replacements[0].status,
        VanillaReplacementStatus::Protected,
        "{} must retain protection across case-variant winners",
        game.id
    );

    let deployed_b =
        deploy_with_backup_dir(&game, &tracker, &cache_root, true, Some(&backup_dir)).await?;
    assert_eq!(deployed_b.vanilla_files_backed_up, 0, "{}", game.id);
    assert_eq!(deployed_b.vanilla_files_restored, 0, "{}", game.id);
    assert_eq!(
        read_live_file(&game, path.case_variant)?,
        mod_b_content,
        "{}",
        game.id
    );
    assert_eq!(
        tracker
            .get_vanilla_backup(&game.id, &canonical_path.to_uppercase())
            .await?,
        Some(original_record.clone()),
        "{} must keep the same original backup record",
        game.id
    );
    assert!(files_match(
        &expected_original,
        &original_record.backup_path
    )?);

    tracker.delete_mod_files("mod-b").await?;
    tracker.delete_mod("mod-b").await?;
    tracker
        .set_setting("protect_vanilla_files", "false")
        .await?;
    assert!(
        !deployment_preflight(&game, &tracker)
            .await?
            .protect_vanilla_files,
        "{} must read the later-disabled protection setting",
        game.id
    );

    let restored = match final_action {
        FinalAction::Deploy => {
            deploy_with_backup_dir(&game, &tracker, &cache_root, false, Some(&backup_dir))
                .await?
                .vanilla_files_restored
        }
        FinalAction::Purge => {
            purge(&game, &tracker, &cache_root)
                .await?
                .vanilla_files_restored
        }
    };
    assert_eq!(
        restored,
        1,
        "{} must report one restoration through {}",
        game.id,
        final_action.name()
    );
    assert_eq!(
        std::fs::read(&live_original)?,
        vanilla_content,
        "{} must restore the byte-exact original through {}",
        game.id,
        final_action.name()
    );
    assert!(files_match(&expected_original, &live_original)?);
    assert!(
        tracker
            .get_vanilla_backup(&game.id, &canonical_path)
            .await?
            .is_none(),
        "{} must clear the restored record",
        game.id
    );
    assert!(
        !original_record.backup_path.exists(),
        "{} must remove the restored payload",
        game.id
    );
    Ok(())
}

fn read_live_file(game: &Game, relative: &str) -> Result<Vec<u8>> {
    let game_data = game::deploy_dir(game);
    let path = find_existing_deploy_path_case_insensitive(relative, &game.path, &game_data)?
        .with_context(|| format!("no deployed file found for '{relative}'"))?;
    std::fs::read(&path).with_context(|| format!("failed to read '{}'", path.display()))
}

async fn run_all_registered_games(final_action: FinalAction) -> Result<BTreeSet<String>> {
    let mut covered = BTreeSet::new();
    let path = LifecyclePath {
        original: "Vanilla/Shared.bin",
        case_variant: "vanilla/shared.BIN",
        backup_anchor: "data",
    };
    for option in known_game_options() {
        run_vanilla_lifecycle(&option, &path, final_action)
            .await
            .with_context(|| {
                format!(
                    "{} vanilla lifecycle failed for {}",
                    final_action.name(),
                    option.deployd_id
                )
            })?;
        covered.insert(option.deployd_id.to_string());
    }
    Ok(covered)
}

fn assert_current_registry_is_covered(covered: &BTreeSet<String>) {
    for game_id in [
        "skyrim-se",
        "fallout-4",
        "fallout-nv",
        "starfield",
        "witcher-3",
        "cyberpunk-2077",
        "witcher-2",
        "witcher-1",
        "dragon-age",
    ] {
        assert!(covered.contains(game_id), "missing lifecycle for {game_id}");
    }
}

// @variants: both
#[tokio::test]
async fn all_registered_games_restore_vanilla_on_deploy() -> Result<()> {
    let covered = run_all_registered_games(FinalAction::Deploy).await?;
    assert_current_registry_is_covered(&covered);
    assert_eq!(covered.len(), known_game_options().len());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn all_registered_games_restore_vanilla_on_purge() -> Result<()> {
    let covered = run_all_registered_games(FinalAction::Purge).await?;
    assert_current_registry_is_covered(&covered);
    assert_eq!(covered.len(), known_game_options().len());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn engine_specific_anchors_preserve_the_shared_lifecycle() -> Result<()> {
    let cases = [
        (
            "skyrim-se",
            LifecyclePath {
                original: "../Root/Protected.bin",
                case_variant: "../root/protected.BIN",
                backup_anchor: "root",
            },
        ),
        (
            "witcher-1",
            LifecyclePath {
                original: "../system/Protected.bin",
                case_variant: "../SYSTEM/protected.BIN",
                backup_anchor: "root",
            },
        ),
        (
            "witcher-1",
            LifecyclePath {
                original: "../launcher/Protected.bin",
                case_variant: "../LAUNCHER/protected.BIN",
                backup_anchor: "root",
            },
        ),
        (
            "witcher-1",
            LifecyclePath {
                original: "../register/Protected.bin",
                case_variant: "../REGISTER/protected.BIN",
                backup_anchor: "root",
            },
        ),
        (
            "dragon-age",
            LifecyclePath {
                original: "~docs~/Settings/Protected.xml",
                case_variant: "~docs~/settings/protected.XML",
                backup_anchor: "docs",
            },
        ),
    ];

    let options = known_game_options();
    for (game_id, path) in cases {
        let option = options
            .iter()
            .find(|option| option.deployd_id == game_id)
            .with_context(|| format!("known game '{game_id}' is missing"))?;
        run_vanilla_lifecycle(option, &path, FinalAction::Deploy)
            .await
            .with_context(|| format!("anchor lifecycle failed for {game_id}: {}", path.original))?;
    }
    Ok(())
}
