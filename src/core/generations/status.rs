use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::core::{save_manager::SaveSetId, tracker::Tracker};
use crate::models::game::Game;

use super::manifest::Manifest;
use super::records::{self, Rows, Table};
use super::state;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) needs_deploy: bool,
    pub(crate) deployed_profile: Option<String>,
    pub(crate) live_saves: Option<SaveSetId>,
    pub(crate) recovery_pending: bool,
}

pub(crate) async fn read(
    tracker: &Tracker,
    game: &Game,
    profile: &str,
    cache: &Path,
) -> Result<Status> {
    let mut tx = tracker.pool.begin().await?;
    let previous = state::read(&mut tx, &game.id).await?;
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id=?)")
            .bind(&game.id)
            .fetch_one(&mut *tx)
            .await?;
    let selected: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM profiles WHERE id=? AND game_id=? AND is_active=1)",
    )
    .bind(profile)
    .bind(&game.id)
    .fetch_one(&mut *tx)
    .await?;
    anyhow::ensure!(
        selected,
        "Selected profile changed while checking deployment"
    );
    let current = records::capture_state(&mut tx, &game.id, profile, true).await?;
    let historical: Option<String> = match previous
        .as_ref()
        .and_then(|state| state.generation.as_deref())
    {
        Some(id) => Some(
            sqlx::query_scalar("SELECT manifest FROM generations WHERE game_id=? AND id=?")
                .bind(&game.id)
                .bind(id)
                .fetch_one(&mut *tx)
                .await?,
        ),
        None => None,
    };
    tx.rollback().await?;
    let mut status = Status {
        deployed_profile: previous.as_ref().and_then(|state| state.profile.clone()),
        live_saves: previous.as_ref().map(|state| state.saves.clone()),
        recovery_pending: pending,
        needs_deploy: pending || previous.as_ref().is_some_and(|state| state.modified),
    };
    if let Some(document) = historical {
        let manifest: Manifest =
            serde_json::from_str(&document).context("Cannot read deployed generation")?;
        manifest.validate()?;
        status.needs_deploy |= status.deployed_profile.as_deref() != Some(profile)
            || projection(&current)? != projection(&manifest.records)?;
        if !status.needs_deploy {
            status.needs_deploy =
                !super::manifest::sources_match(tracker, game, cache, &manifest).await?;
        }
    } else {
        let isolated =
            current[0].rows[0].get("save_mode").and_then(Value::as_str) == Some("profile");
        let has_mods = current
            .iter()
            .any(|rows| rows.table == Table::Mods && !rows.rows.is_empty());
        status.needs_deploy |= if let Some(deployed) = &status.deployed_profile {
            has_mods
                || deployed != profile
                || isolated
                    && status.live_saves.as_ref().and_then(SaveSetId::profile_id) != Some(profile)
        } else {
            has_mods || isolated
        };
    }
    Ok(status)
}

pub(super) fn projection(rows: &[Rows]) -> Result<Vec<(&'static str, BTreeSet<String>)>> {
    let mut rows = rows.to_vec();
    for table in &mut rows {
        if table.table != Table::MelePackages {
            continue;
        }
        for row in &mut table.rows {
            let mut record: crate::core::game::mass_effect::library::Record =
                serde_json::from_str(records::text(row, "document")?)
                    .context("Cannot read MELE package deployment status")?;
            // The mods table owns profile selection; this copy reflects import-time state.
            record.package.enabled = false;
            row.insert(
                "document".into(),
                Value::String(serde_json::to_string(&record)?),
            );
        }
    }
    Ok(rows
        .iter()
        .filter_map(|rows| {
            let fields: &[&str] = match rows.table {
                Table::Profiles => &["save_mode"],
                Table::Groups => return None,
                Table::Mods => &[
                    "id",
                    "enabled",
                    "priority",
                    "install_target",
                    "archive_hash",
                ],
                Table::Files => &["mod_id", "game_rel_lowercase", "game_rel_original"],
                Table::Plugins => &["id", "mod_id", "filename", "enabled", "load_order"],
                Table::Masters => &["plugin_id", "master_name", "master"],
                Table::ProfileMods | Table::ProfilePlugins => return None,
                Table::MelePackages => &["mod_id", "document"],
                Table::MeleRecipes => &["document"],
            };
            Some((
                rows.table.name(),
                rows.rows
                    .iter()
                    .map(|row| {
                        format!(
                            "{:?}",
                            fields.iter().map(|key| row.get(*key)).collect::<Vec<_>>()
                        )
                    })
                    .collect(),
            ))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::generations::{activation, content::Control};
    use crate::models::game::GameEngine;

    // @variants: both
    #[test]
    fn mele_import_flags_do_not_hide_profile_or_package_changes() -> Result<()> {
        let document = serde_json::json!({
            "version": 1, "target": "LE2",
            "package": {
                "id": "package", "source_sha256": "source", "archive_sha256": null,
                "manifest_version": "9.1", "mod_version": "1.0",
                "enabled": true, "options": []
            }
        });
        let mut current = vec![
            Rows {
                table: Table::Mods,
                rows: vec![records::Record::from([
                    ("id".into(), Value::from("package")),
                    ("enabled".into(), Value::from(0)),
                    ("priority".into(), Value::from(1)),
                ])],
            },
            Rows {
                table: Table::MelePackages,
                rows: vec![records::Record::from([
                    ("mod_id".into(), Value::from("package")),
                    ("document".into(), Value::from(document.to_string())),
                ])],
            },
        ];
        let mut deployed = current.clone();
        let mut document = document;
        document["package"]["enabled"] = Value::from(false);
        deployed[1].rows[0].insert("document".into(), Value::from(document.to_string()));
        assert_eq!(projection(&current)?, projection(&deployed)?);

        for (field, value) in [("enabled", 1), ("priority", 2)] {
            let original = current[0].rows[0].insert(field.into(), Value::from(value));
            assert_ne!(projection(&current)?, projection(&deployed)?);
            current[0].rows[0].insert(field.into(), original.context("Missing fixture field")?);
        }
        assert_eq!(projection(&current)?, projection(&deployed)?);

        document["package"]["options"] = serde_json::json!(["alternative"]);
        current[1].rows[0].insert("document".into(), Value::from(document.to_string()));
        assert_ne!(projection(&current)?, projection(&deployed)?);
        current[1].rows[0].insert("document".into(), Value::from("invalid"));
        assert!(projection(&current).is_err());
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn deployment_status_tracks_reverted_drafts_and_cache_changes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, mut game, profile) =
            crate::core::generations::tests::snapshot_fixture(temp.path()).await?;
        game.engine = GameEngine::Aurora;
        std::fs::create_dir_all(game.path.join("data"))?;
        tracker.switch_profile(&game.id, &profile).await?;
        assert!(
            read(&tracker, &game, &profile, temp.path())
                .await?
                .needs_deploy
        );
        tracker.save_to_profile(&profile, &game.id).await?;
        activation::prepare(
            &tracker,
            &game,
            temp.path(),
            Some(&profile),
            false,
            Control::default(),
        )
        .await?
        .activate(Control::default())
        .await?;
        assert!(
            !read(&tracker, &game, &profile, temp.path())
                .await?
                .needs_deploy
        );
        sqlx::query("UPDATE mods SET enabled=0 WHERE id='winner'")
            .execute(&tracker.pool)
            .await?;
        assert!(
            read(&tracker, &game, &profile, temp.path())
                .await?
                .needs_deploy
        );
        sqlx::query("UPDATE mods SET enabled=1 WHERE id='winner'")
            .execute(&tracker.pool)
            .await?;
        assert!(
            !read(&tracker, &game, &profile, temp.path())
                .await?
                .needs_deploy
        );
        std::fs::write(temp.path().join("winner/file.txt"), b"tool change")?;
        assert!(
            read(&tracker, &game, &profile, temp.path())
                .await?
                .needs_deploy
        );
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn untouched_game_stays_neutral_until_its_save_mode_changes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, game, profile) =
            crate::core::generations::tests::snapshot_fixture(temp.path()).await?;
        tracker.switch_profile(&game.id, &profile).await?;
        sqlx::query("DELETE FROM mods")
            .execute(&tracker.pool)
            .await?;
        assert!(
            !read(&tracker, &game, &profile, temp.path())
                .await?
                .needs_deploy
        );
        sqlx::query("UPDATE profiles SET save_mode='profile' WHERE id=?")
            .bind(&profile)
            .execute(&tracker.pool)
            .await?;
        assert!(
            read(&tracker, &game, &profile, temp.path())
                .await?
                .needs_deploy
        );
        Ok(())
    }
}
