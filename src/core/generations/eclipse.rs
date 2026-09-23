use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use anyhow::{Context, Result, ensure};

use crate::core::game::eclipse::generations as engine;
use crate::models::game::{Game, GameEngine};

use super::catalog::History;
use super::content::Control;
use super::journal::{Journal, Node};
use super::manifest::{Manifest, Output};
use super::target::Target;

pub(super) fn configuration(target: &Target) -> bool {
    matches!(target, Target::Eclipse { documents: false, path } if path.eq_ignore_ascii_case("settings/addins.xml"))
}

fn registration(target: &Target) -> bool {
    matches!(target, Target::Eclipse { documents: false, path } if path.to_lowercase().starts_with("addins/") && path.to_lowercase().ends_with("/manifest.xml"))
}

pub(super) async fn prepare(
    history: &History,
    game: &Game,
    manifest: &mut Option<Manifest>,
    journal: &mut Journal,
    control: Control,
) -> Result<()> {
    ensure!(
        game.engine == GameEngine::Eclipse,
        "Add-in registration belongs to Eclipse"
    );
    let existing = journal.changes.clone();
    let outputs = manifest
        .as_ref()
        .map(|manifest| manifest.outputs.clone())
        .unwrap_or_default();
    let configured = outputs
        .iter()
        .find(|output| configuration(&output.target))
        .cloned();
    let target = configured
        .as_ref()
        .map(|output| output.target.clone())
        .unwrap_or_else(|| Target::Eclipse {
            documents: false,
            path: "Settings/AddIns.xml".into(),
        });
    let mut addition = Journal::prepare(
        history,
        game,
        vec![(target.clone(), Node::Absent)],
        control.clone(),
    )
    .await?;
    let before = addition
        .changes
        .first()
        .context("Add-in configuration inspection is missing")?
        .before
        .clone();
    let initial = configured
        .as_ref()
        .and_then(|output| output.content.clone())
        .or_else(|| match &before {
            Node::File { identity, .. } => Some(identity.clone()),
            _ => None,
        });
    let store = history.store.clone();
    let reading = control.clone();
    let (bytes, needed) = history
        .lease
        .blocking(move || -> Result<_> {
            let mut previous = BTreeSet::new();
            for change in existing
                .iter()
                .filter(|change| registration(&change.target))
            {
                if let Node::File { identity, .. } = &change.before {
                    store.verify(identity, &reading)?;
                    previous.extend(
                        engine::registrations(&fs::read(store.source(identity)?)?)?.into_keys(),
                    );
                }
            }
            let mut desired = BTreeMap::new();
            for output in outputs.iter().filter(|output| registration(&output.target)) {
                let identity = output
                    .content
                    .as_ref()
                    .context("Add-in manifest is not a file")?;
                store.verify(identity, &reading)?;
                for (uid, block) in engine::registrations(&fs::read(store.source(identity)?)?)? {
                    ensure!(
                        desired.insert(uid, block).is_none(),
                        "Enabled add-ins contain duplicate registration UIDs"
                    );
                }
            }
            let needed = !previous.is_empty() || !desired.is_empty();
            let current = initial
                .map(|identity| -> Result<_> {
                    store.verify(&identity, &reading)?;
                    Ok(fs::read_to_string(store.source(&identity)?)?)
                })
                .transpose()?;
            Ok((
                if needed {
                    engine::render(current.as_deref(), &previous, &desired)
                        .context("Cannot update Settings/AddIns.xml; restore a valid backup if its XML is damaged")?
                        .into_bytes()
                } else {
                    Vec::new()
                },
                needed,
            ))
        })
        .await
        .context("Add-in preparation worker stopped")??;
    if !needed {
        return Ok(());
    }
    let content = history.retain_generated(bytes, control.clone()).await?;
    let mode = configured
        .as_ref()
        .map(|output| output.mode)
        .unwrap_or(match before {
            Node::File { mode, .. } => mode,
            _ => 0o644,
        });
    let output = Output {
        target: target.clone(),
        content: Some(content.clone()),
        mode,
        mod_id: None,
    };
    let change = addition
        .changes
        .first_mut()
        .context("Add-in configuration inspection is missing")?;
    change.after = Node::File {
        identity: content,
        mode,
    };
    if let Some(existing) = journal
        .changes
        .iter_mut()
        .find(|change| configuration(&change.target))
    {
        ensure!(
            existing.before == change.before,
            "AddIns.xml changed during preparation"
        );
        *existing = change.clone();
    } else {
        journal
            .extend(history, game, vec![(target, change.after.clone())], control)
            .await?;
    }
    if let Some(manifest) = manifest {
        manifest
            .outputs
            .retain(|output| !configuration(&output.target));
        manifest.outputs.push(output);
        manifest.outputs.sort_by(|a, b| a.target.cmp(&b.target));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[tokio::test]
    async fn deployment_preserves_dlc_registration_and_rollback_restores_original_bytes()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, mut game, profile) =
            super::super::tests::snapshot_fixture(temp.path()).await?;
        game.engine = GameEngine::Eclipse;
        game.id = "dragon-age".into();
        game.data_subdir = "Documents/BioWare/Dragon Age".into();
        sqlx::query("UPDATE profiles SET game_id='dragon-age'")
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE mods SET game_id='dragon-age'")
            .execute(&tracker.pool)
            .await?;
        game.wine_prefix = Some(temp.path().join("prefix"));
        fs::create_dir_all(temp.path().join("prefix/drive_c/users/steamuser/Documents"))?;
        fs::write(temp.path().join("prefix/system.reg"), "")?;
        fs::create_dir_all(crate::core::game::deploy_dir(&game).join("Settings"))?;
        let xml = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<AddInsList>\r\n<AddInItem UID=\"official\"><Name>DLC</Name></AddInItem>\r\n</AddInsList>";
        let config = crate::core::game::deploy_dir(&game).join("Settings/AddIns.xml");
        fs::write(&config, xml)?;
        fs::write(
            temp.path().join("winner/file.txt"),
            "\u{feff}<Manifest><AddInItem UID=\"mod\"><Name>Mod</Name></AddInItem></Manifest>",
        )?;
        sqlx::query("UPDATE mod_files SET game_rel_lowercase='addins/mod/manifest.xml',game_rel_original='AddIns/mod/Manifest.xml'")
            .execute(&tracker.pool).await?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let (manifest, _, mut journal) =
            super::super::state_tests::unpublished(&history, &game, &profile).await?;
        let mut manifest = Some(manifest);
        prepare(
            &history,
            &game,
            &mut manifest,
            &mut journal,
            Control::default(),
        )
        .await?;
        journal
            .verify_prepared(&history, &game, Vec::new(), Control::default())
            .await?;
        journal
            .persist(&history, &game, "deploy", Default::default())
            .await?;
        let _applied = journal.apply(&history, &game, Control::default()).await?;
        let registered = engine::registrations(&fs::read(&config)?)?;
        assert_eq!(registered.len(), 2);
        assert_eq!(
            registered["official"],
            "<AddInItem UID=\"official\"><Name>DLC</Name></AddInItem>"
        );
        assert!(registered.contains_key("mod"));
        journal.recover(&history, &game, false).await?;
        assert_eq!(fs::read_to_string(config)?, xml);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn already_absent_old_targets_do_not_require_recreating_their_parents() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, mut game, _) = super::super::tests::snapshot_fixture(temp.path()).await?;
        game.engine = GameEngine::Eclipse;
        game.id = "dragon-age".into();
        game.data_subdir = "Documents/BioWare/Dragon Age".into();
        sqlx::query("UPDATE profiles SET game_id='dragon-age'")
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE mods SET game_id='dragon-age'")
            .execute(&tracker.pool)
            .await?;
        game.wine_prefix = Some(temp.path().join("prefix"));
        fs::create_dir_all(temp.path().join("prefix/drive_c/users/steamuser/Documents"))?;
        fs::write(temp.path().join("prefix/system.reg"), "")?;
        fs::create_dir_all(crate::core::game::deploy_dir(&game))?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let target = Target::file(&game.engine, "AddIns/removed/Manifest.xml")?;
        let journal = Journal::prepare(
            &history,
            &game,
            vec![(target.clone(), Node::Absent)],
            Control::default(),
        )
        .await?;
        journal
            .verify_prepared(&history, &game, Vec::new(), Control::default())
            .await?;
        journal
            .persist(&history, &game, "purge", Default::default())
            .await?;
        let _applied = journal.apply(&history, &game, Control::default()).await?;
        journal.recover(&history, &game, false).await?;
        assert!(!crate::core::game::deploy_dir(&game).join("AddIns").exists());
        fs::create_dir_all(crate::core::game::deploy_dir(&game).join("AddIns/removed"))?;
        fs::write(target.resolve(&game)?, b"external")?;
        assert!(
            journal
                .verify_prepared(&history, &game, Vec::new(), Control::default())
                .await
                .is_err()
        );
        fs::remove_dir_all(temp.path().join("prefix"))?;
        assert!(
            journal
                .verify_prepared(&history, &game, Vec::new(), Control::default())
                .await
                .is_err()
        );
        Ok(())
    }
}
