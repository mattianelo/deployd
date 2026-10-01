use super::*;

pub(in crate::core::tracker) async fn reconcile(connection: &mut SqliteConnection) -> Result<()> {
    let detached: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT l.id FROM folder_locations l JOIN game_locations g ON g.location_id=l.id JOIN games game ON game.id=g.game_id WHERE g.role='game' AND game.engine='mass_effect' AND l.host_hint IS NOT NULL AND NOT EXISTS(SELECT 1 FROM mele_families f WHERE f.location_id=l.id)")
        .fetch_all(&mut *connection).await?;
    for id in detached {
        let current = read_location(connection, id).await?;
        let candidates: Vec<i64> = sqlx::query_scalar("SELECT l.id FROM folder_locations l JOIN mele_families f ON f.location_id=l.id WHERE l.host_hint=? AND l.id<>?")
            .bind(current.selection.host_hint.as_deref().map(path_text).transpose()?)
            .bind(id).fetch_all(&mut *connection).await?;
        let canonical = match candidates.as_slice() {
            [] => continue,
            [id] => *id,
            _ => bail!(
                "Multiple launcher records match this installation; restore its original folder access before deploying"
            ),
        };
        let original = read_location(connection, canonical).await?;
        if !current.selection.validate_identity(&original.selection)? {
            continue;
        }
        if !original.bindings.is_empty() {
            bail!(
                "The original trilogy location still has game bindings; restore its shared folder access before changing folders"
            );
        }
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM location_repairs WHERE location_id IN (?,?))",
        )
        .bind(id)
        .bind(canonical)
        .fetch_one(&mut *connection)
        .await?;
        if pending {
            bail!("Finish pending folder recovery before reconnecting trilogy launcher records");
        }
        let mut changes = Vec::new();
        for binding in &current.bindings {
            let number = match binding.game_id.as_str() {
                "mass-effect-le1" => 1,
                "mass-effect-le2" => 2,
                "mass-effect-le3" => 3,
                _ => bail!(
                    "The trilogy folder is shared with another game; restore its original folder binding"
                ),
            };
            if binding.role != FolderRole::Game
                || binding.relative != Path::new(&format!("Game/ME{number}"))
            {
                bail!("Select the shared Legendary Edition installation folder in Manage Games");
            }
            let old_path = resolve_relative(&original.selection.root, &binding.relative)?;
            let new_path = resolve_relative(&current.selection.root, &binding.relative)?;
            rebase_tool_paths(connection, &binding.game_id, &old_path, &new_path).await?;
            changes.push(FolderChange {
                game_id: binding.game_id.clone(),
                role: binding.role,
                old_path,
                new_path,
            });
        }
        sqlx::query("UPDATE game_locations SET location_id=? WHERE location_id=?")
            .bind(canonical)
            .bind(id)
            .execute(&mut *connection)
            .await?;
        sqlx::query("UPDATE mele_profile_groups SET location_id=? WHERE location_id=?")
            .bind(canonical)
            .bind(id)
            .execute(&mut *connection)
            .await?;
        sqlx::query("UPDATE folder_locations SET root=?,host_hint=? WHERE id=?")
            .bind(path_text(&current.selection.root)?)
            .bind(
                current
                    .selection
                    .host_hint
                    .as_deref()
                    .map(path_text)
                    .transpose()?,
            )
            .bind(canonical)
            .execute(&mut *connection)
            .await?;
        if crate::utils::portal::is_document_path(&original.selection.root) {
            sqlx::query("INSERT INTO location_repairs(location_id,changes) VALUES (?,?)")
                .bind(canonical)
                .bind(serde_json::to_string(&changes)?)
                .execute(&mut *connection)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::game::mass_effect::family::Family;
    use crate::models::game::{Game, GameConfig, GameEngine};

    async fn detached(tracker: &Tracker, same_hint: bool) -> Result<(i64, i64, Family)> {
        let configs: Vec<_> = (1..=3)
            .map(|number| GameConfig {
                game: Game {
                    id: format!("mass-effect-le{number}"),
                    title: format!("LE{number}"),
                    path: format!("/run/user/1000/doc/old/MELE/Game/ME{number}").into(),
                    data_subdir: "BioGame".into(),
                    engine: GameEngine::MassEffect,
                    wine_prefix: None,
                },
                custom: true,
                locations: vec![FolderSelection {
                    role: FolderRole::Game,
                    location: SelectedLocation {
                        root: "/run/user/1000/doc/old/MELE".into(),
                        host_hint: Some("/games/MELE".into()),
                    },
                    relative: format!("Game/ME{number}").into(),
                }],
            })
            .collect();
        tracker.persist_game_configs(&configs, &[]).await?;
        let old = tracker
            .folder_location("mass-effect-le1", FolderRole::Game)
            .await?
            .id;
        let family: Family = serde_json::from_value(
            serde_json::json!({"version":1,"original":{"size":8,"sha256":"a".repeat(64)},"owners":["mass-effect-le1","mass-effect-le3"],"installed":true}),
        )?;
        tracker.record_mele_family(old, &family).await?;
        tracker.ensure_default_profile("mass-effect-le1").await?;
        let new = sqlx::query(
            "INSERT INTO folder_locations(root,host_hint) VALUES ('/run/user/1000/doc/new/MELE',?)",
        )
        .bind(if same_hint {
            "/games/MELE"
        } else {
            "/games/Other"
        })
        .execute(&tracker.pool)
        .await?
        .last_insert_rowid();
        sqlx::query("UPDATE game_locations SET location_id=? WHERE location_id=?")
            .bind(new)
            .bind(old)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE games SET path=replace(path,'/doc/old/','/doc/new/')")
            .execute(&tracker.pool)
            .await?;
        Ok((old, new, family))
    }

    // @variants: both
    #[tokio::test]
    async fn reconnects_the_existing_launcher_and_profiles_after_portal_reselection() -> Result<()>
    {
        let directory = tempfile::tempdir()?;
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("tracker.db").display()
        );
        let tracker = Tracker::open(&url).await?.tracker;
        let (old, _, family) = detached(&tracker, true).await?;
        tracker.pool.close().await;
        let tracker = Tracker::open(&url).await?.tracker;
        assert_eq!(tracker.mele_family(old).await?, Some(family));
        for number in 1..=3 {
            let location = tracker
                .folder_location(&format!("mass-effect-le{number}"), FolderRole::Game)
                .await?;
            assert_eq!(location.id, old);
            assert_eq!(
                location.selection.root,
                Path::new("/run/user/1000/doc/new/MELE")
            );
        }
        let groups: Vec<i64> = sqlx::query_scalar("SELECT location_id FROM mele_profile_groups")
            .fetch_all(&tracker.pool)
            .await?;
        assert_eq!(groups, vec![old]);
        let repairs = tracker.pending_location_repairs().await?;
        assert_eq!(repairs.len(), 1);
        assert_eq!(repairs[0].location_id, old);
        assert_eq!(repairs[0].changes.len(), 3);
        let mut tx = tracker.pool.begin().await?;
        reconcile(&mut tx).await?;
        tx.commit().await?;
        tracker.pool.close().await;
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn saving_a_reselected_trilogy_folder_preserves_its_launcher_identity() -> Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let (old, new, family) = detached(&tracker, true).await?;
        let configs: Vec<_> = tracker
            .load_persisted_games()
            .await?
            .into_iter()
            .map(|game| GameConfig {
                game: Game {
                    id: game.id,
                    title: game.title,
                    path: game.path,
                    data_subdir: game.data_subdir,
                    engine: game.engine,
                    wine_prefix: game.wine_prefix,
                },
                custom: game.custom,
                locations: vec![],
            })
            .collect();
        sqlx::query("UPDATE game_locations SET location_id=? WHERE location_id=?")
            .bind(old)
            .bind(new)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE games SET path=replace(path,'/doc/new/','/doc/old/')")
            .execute(&tracker.pool)
            .await?;
        let configs: Vec<_> = configs
            .into_iter()
            .map(|mut config| {
                config.locations.push(FolderSelection {
                    role: FolderRole::Game,
                    location: SelectedLocation {
                        root: "/run/user/1000/doc/new/MELE".into(),
                        host_hint: Some("/games/MELE".into()),
                    },
                    relative: PathBuf::from("Game")
                        .join(format!("ME{}", config.game.id.chars().last().unwrap())),
                });
                config
            })
            .collect();
        tracker.persist_game_configs(&configs, &[]).await?;
        for config in configs {
            assert_eq!(
                tracker
                    .folder_location(&config.game.id, FolderRole::Game)
                    .await?
                    .id,
                old
            );
        }
        assert_eq!(tracker.mele_family(old).await?, Some(family));
        assert!(tracker.mele_family(new).await?.is_none());
        assert_eq!(tracker.pending_location_repairs().await?.len(), 1);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn leaves_ambiguous_or_pending_folder_reconnections_untouched() -> Result<()> {
        for pending in [false, true] {
            let tracker = Tracker::open("sqlite::memory:").await?.tracker;
            let (old, new, family) = detached(&tracker, true).await?;
            if pending {
                sqlx::query("INSERT INTO location_repairs(location_id,changes) VALUES (?,'[]')")
                    .bind(old)
                    .execute(&tracker.pool)
                    .await?;
            } else {
                let other = sqlx::query("INSERT INTO folder_locations(root,host_hint) VALUES ('/another/grant','/games/MELE')").execute(&tracker.pool).await?.last_insert_rowid();
                tracker.record_mele_family(other, &family).await?;
            }
            let mut tx = tracker.pool.begin().await?;
            assert!(reconcile(&mut tx).await.is_err());
            tx.rollback().await?;
            assert_eq!(
                tracker
                    .folder_location("mass-effect-le1", FolderRole::Game)
                    .await?
                    .id,
                new
            );
            assert_eq!(tracker.mele_family(old).await?, Some(family));
        }
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn preserves_unrelated_installations_and_rolls_back_failed_reconnections() -> Result<()> {
        for same_hint in [false, true] {
            let tracker = Tracker::open("sqlite::memory:").await?.tracker;
            let (old, new, family) = detached(&tracker, same_hint).await?;
            sqlx::query("CREATE TRIGGER reject_rebind BEFORE UPDATE ON game_locations BEGIN SELECT RAISE(FAIL,'injected'); END").execute(&tracker.pool).await?;
            let mut tx = tracker.pool.begin().await?;
            let result = reconcile(&mut tx).await;
            assert_eq!(result.is_err(), same_hint);
            tx.rollback().await?;
            assert_eq!(
                tracker
                    .folder_location("mass-effect-le1", FolderRole::Game)
                    .await?
                    .id,
                new
            );
            assert_eq!(tracker.mele_family(old).await?, Some(family));
            assert!(tracker.pending_location_repairs().await?.is_empty());
        }
        Ok(())
    }
}
