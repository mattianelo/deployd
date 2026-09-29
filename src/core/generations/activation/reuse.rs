use std::collections::BTreeMap;

use serde_json::Value;

use super::*;
use crate::core::game::mass_effect::{application::Request, family, library};
use crate::core::generations::{records, status};

pub(crate) async fn prepare_unchanged(
    tracker: &Tracker,
    cache: &Path,
    request: &Request,
    control: Control,
) -> Result<Option<Prepared>> {
    if request.purge || request.repair || request.game.engine != GameEngine::MassEffect {
        return Ok(None);
    }
    let game = &request.game;
    let profile = &request.profile;
    let history = History::open(tracker, &game.id, cache, true).await?;
    recovery::recover(&history, game).await?;
    control.check()?;
    let mut tx = tracker.pool.begin().await?;
    let Some(previous) = state::read(&mut tx, &game.id).await? else {
        return Ok(None);
    };
    if previous.modified {
        return Ok(None);
    }
    let Some(generation) = &previous.generation else {
        return Ok(None);
    };
    let mut current = records::capture_state(&mut tx, &game.id, profile, true).await?;
    tx.rollback().await?;
    let mut manifest = history
        .load_with_control(generation, control.clone())
        .await?;
    let Some(snapshot) = &manifest.mele else {
        return Ok(None);
    };
    let desired = library::desired(tracker, game, request.language.clone(), false).await?;
    if desired != snapshot.recipe || !same_mods(&current, &manifest.records)? {
        return Ok(None);
    }
    if !manifest::sources_match(tracker, game, cache, &manifest).await? {
        return Ok(None);
    }
    ensure!(
        tracker
            .get_active_profile(&game.id)
            .await?
            .is_some_and(|active| active.id == *profile),
        "The selected profile changed; retry Deploy"
    );
    let engine =
        crate::core::game::mass_effect::journal::Journal::reselect(tracker, game, profile.clone())
            .await?;
    ensure!(
        engine.desired.recipe.as_ref() == Some(&snapshot.recipe)
            && engine.desired.files == snapshot.files
            && engine.desired.removals == snapshot.removals,
        "Installed MELE outputs differ from their retained generation; prepare a fresh deployment"
    );
    let verifying = snapshot.clone();
    let checking_tracker = tracker.clone();
    let checking_game = game.clone();
    let checking_previous = engine.previous.clone();
    history
        .lease
        .participant(async move {
            verifying
                .verify_inputs(
                    &checking_tracker,
                    &checking_game,
                    checking_previous.as_ref(),
                )
                .await
        })
        .await
        .context("MELE input verification stopped")??;
    let recipe_rows = current
        .iter_mut()
        .find(|rows| rows.table == records::Table::MeleRecipes)
        .context("Missing MELE recipe table")?;
    recipe_rows.rows = vec![BTreeMap::from([
        ("game_id".into(), Value::String(game.id.clone())),
        ("profile_id".into(), Value::String(profile.clone())),
        (
            "document".into(),
            Value::String(serde_json::to_string(&snapshot.recipe)?),
        ),
    ])];
    manifest.records = current;
    let targets = manifest
        .outputs
        .iter()
        .map(|output| {
            Ok((
                output.target.clone(),
                Node::File {
                    identity: output
                        .content
                        .clone()
                        .context("Missing retained MELE output")?,
                    mode: output.mode,
                },
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut journal = Journal::prepare(&history, game, targets, control.clone()).await?;
    ensure!(
        journal
            .changes
            .iter()
            .all(|change| change.before == change.after),
        "Installed MELE files changed; reconcile external edits before switching profiles"
    );
    journal.attach_mele(game, engine)?;
    let checking_tracker = tracker.clone();
    let checking_game = game.clone();
    let desired = journal
        .mele
        .as_ref()
        .context("Missing MELE participant")?
        .desired
        .clone();
    journal.dependency = history
        .lease
        .participant(async move {
            family::generations::dependency(&checking_tracker, &checking_game, &desired).await
        })
        .await
        .context("MELE shared dependency verification stopped")??;
    manifest.shared_revision = super::super::shared::identity(journal.dependency.as_ref())?;
    manifest.validate()?;
    let mut target_saves = previous.saves.clone();
    if game::has_save_management(game) {
        let prepared = saves::prepare(&history, game, journal, &manifest, control).await?;
        journal = prepared.journal;
        target_saves = prepared.target;
    }
    Ok(Some(Prepared {
        history,
        game: game.clone(),
        journal,
        previous,
        manifest: Some(manifest),
        files: Vec::new(),
        saves: target_saves,
        differences: Vec::new(),
        outcome: empty_outcome(),
    }))
}

fn same_mods(current: &[records::Rows], deployed: &[records::Rows]) -> Result<bool> {
    let mods = |rows| -> Result<Vec<_>> {
        Ok(status::projection(rows)?
            .into_iter()
            .filter(|(table, _)| *table != "profiles" && *table != "mele_recipes")
            .collect())
    };
    Ok(mods(current)? == mods(deployed)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use records::{Rows, Table};

    // @variants: both
    #[test]
    fn save_mode_and_profile_identity_do_not_rebuild_mods_but_mod_changes_do() -> Result<()> {
        let profile = |mode: &str| Rows {
            table: Table::Profiles,
            rows: vec![BTreeMap::from([(
                "save_mode".into(),
                Value::String(mode.into()),
            )])],
        };
        assert!(same_mods(&[profile("global")], &[profile("profile")])?);
        let row = |enabled| Rows {
            table: Table::Mods,
            rows: vec![BTreeMap::from([("enabled".into(), Value::Bool(enabled))])],
        };
        assert!(!same_mods(&[row(true)], &[row(false)])?);
        Ok(())
    }
}
