use std::fs;

use anyhow::{Context, Result};

use crate::core::{game, save_manager::SaveSetId};

use super::catalog::History;
use super::content::Control;
use super::coordinator;
use super::journal::{Journal, Node};
use super::manifest::Output;
use super::state::{Deployment, State};
use super::target::Target;

// @variants: both
#[tokio::test]
async fn retained_generated_configuration_commits_or_rolls_back_with_mod_files() -> Result<()> {
    for fail_commit in [true, false] {
        let temp = tempfile::tempdir()?;
        let (tracker, mut game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
        game.id = "skyrim-se".into();
        game.wine_prefix = Some(temp.path().join("prefix"));
        sqlx::query("UPDATE profiles SET game_id=?")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE mods SET game_id=?")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        fs::create_dir_all(game.data_dir())?;
        fs::create_dir_all(temp.path().join("prefix/drive_c/users/player"))?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let previous = State {
            generation: None,
            profile: None,
            saves: SaveSetId::Global {
                game_id: game.id.clone(),
            },
            modified: false,
        };
        sqlx::query(
            "INSERT INTO generation_game_state(game_id,live_save_mode) VALUES (?,'global')",
        )
        .bind(&game.id)
        .execute(&tracker.pool)
        .await?;
        let (mut manifest, files, _) =
            super::state_tests::unpublished(&history, &game, &profile).await?;
        let plugin_paths = game::plugins_txt_paths(&game);
        let ini_paths = game::custom_ini_paths(&game);
        assert_eq!(plugin_paths.len(), 2);
        assert_eq!(ini_paths.len(), 2);
        let old_plugins = b"# original order\nOld.esp\n";
        let new_plugins = b"# This file is managed by Deployd\n*Example.esp\n";
        let old_ini = b"[Display]\niSize W=1920\n";
        let new_ini = b"[Display]\niSize W=1920\n[Archive]\nbInvalidateOlderFiles=1\nsResourceDataDirsFinal=\n";
        for (paths, old, new, plugin) in [
            (
                &plugin_paths,
                old_plugins.as_slice(),
                new_plugins.as_slice(),
                true,
            ),
            (&ini_paths, old_ini.as_slice(), new_ini.as_slice(), false),
        ] {
            let content = history
                .retain_generated(new.to_vec(), Control::default())
                .await?;
            for (slot, path) in paths.iter().enumerate() {
                fs::create_dir_all(path.parent().context("Configuration parent")?)?;
                fs::write(path, old)?;
                manifest.outputs.push(Output {
                    target: if plugin {
                        Target::PluginControl { slot }
                    } else {
                        Target::CustomIni { slot }
                    },
                    content: Some(content.clone()),
                    mode: 0o644,
                    mod_id: None,
                });
            }
        }
        let desired = manifest
            .outputs
            .iter()
            .map(|output| {
                Ok((
                    output.target.clone(),
                    Node::File {
                        identity: output.content.clone().context("Prepared content")?,
                        mode: output.mode,
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let journal = Journal::prepare(&history, &game, desired, Control::default()).await?;
        for path in &plugin_paths {
            assert_eq!(fs::read(path)?, old_plugins);
        }
        for path in &ini_paths {
            assert_eq!(fs::read(path)?, old_ini);
        }
        assert!(!game.data_dir().join("File.txt").exists());
        if fail_commit {
            sqlx::query("CREATE TRIGGER reject_configuration BEFORE INSERT ON generation_activations BEGIN SELECT RAISE(ABORT,'injected failure'); END")
                .execute(&tracker.pool).await?;
        }
        let result = coordinator::activate(
            &history,
            &game,
            &journal,
            Some(&previous),
            Some(&Deployment {
                manifest: &manifest,
                profile: &profile,
                files: &files,
            }),
            &SaveSetId::Global {
                game_id: game.id.clone(),
            },
            Control::default(),
        )
        .await;
        assert_eq!(result.is_err(), fail_commit, "{result:?}");
        for path in &plugin_paths {
            assert_eq!(
                fs::read(path)?,
                if fail_commit {
                    old_plugins.as_slice()
                } else {
                    new_plugins.as_slice()
                }
            );
        }
        for path in &ini_paths {
            assert_eq!(
                fs::read(path)?,
                if fail_commit {
                    old_ini.as_slice()
                } else {
                    new_ini.as_slice()
                }
            );
        }
        assert_eq!(game.data_dir().join("File.txt").exists(), !fail_commit);
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(pending, 0);
        if !fail_commit {
            let stored = history.load(&manifest.id()?).await?;
            assert_eq!(stored, manifest);
            fs::write(&plugin_paths[0], b"external tool edit")?;
            assert_eq!(history.load(&manifest.id()?).await?, manifest);
        }
    }
    Ok(())
}
