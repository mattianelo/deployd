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
    let mut manifest = history.load_manifest(generation).await?;
    if manifest.version < 2 {
        return Ok(None);
    }
    normalize_routes(
        &mut current,
        cache,
        &crate::utils::paths::deployd_data_dir()?,
    )?;
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
    history
        .load_with_control(generation, control.clone())
        .await?;
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

fn normalize_routes(tables: &mut [records::Rows], cache: &Path, data: &Path) -> Result<()> {
    let mut roots = BTreeMap::new();
    for row in tables
        .iter()
        .filter(|rows| rows.table == records::Table::MelePackages)
        .flat_map(|rows| &rows.rows)
    {
        let record: library::Record = serde_json::from_str(records::text(row, "document")?)?;
        record.validate()?;
        let id = records::text(row, "mod_id")?;
        roots.insert(
            id.to_owned(),
            if record.writable_cache {
                cache.join(id)
            } else {
                data.join("mele-sources").join(record.package.source_sha256)
            },
        );
    }
    for row in tables
        .iter_mut()
        .filter(|rows| rows.table == records::Table::Files)
        .flat_map(|rows| &mut rows.rows)
    {
        let id = records::text(row, "mod_id")?;
        let root = roots
            .get(id)
            .context("MELE file routing has no source package")?;
        let suffix = Path::new(records::text(row, "cache_path")?)
            .strip_prefix(root)
            .context(
                "MELE file routing is outside its source package; reinstall the affected mod",
            )?;
        let mut logical = Path::new("cache").join(id);
        if !suffix.as_os_str().is_empty() {
            logical.push(suffix);
        }
        row.insert(
            "cache_path".into(),
            Value::String(
                logical
                    .to_str()
                    .context("MELE source path is not UTF-8")?
                    .to_owned(),
            ),
        );
    }
    Ok(())
}

fn same_mods(current: &[records::Rows], deployed: &[records::Rows]) -> Result<bool> {
    let mods = |rows| -> Result<Vec<_>> {
        Ok(status::projection(rows)?
            .into_iter()
            .filter(|(table, _)| *table != "profiles" && *table != "mele_recipes")
            .collect())
    };
    let routes = |tables: &[records::Rows]| -> Result<BTreeMap<_, _>> {
        tables
            .iter()
            .filter(|rows| rows.table == records::Table::Files)
            .flat_map(|rows| &rows.rows)
            .map(|row| {
                Ok((
                    (
                        records::text(row, "mod_id")?.to_owned(),
                        records::text(row, "game_rel_lowercase")?.to_owned(),
                    ),
                    records::text(row, "cache_path")?.to_owned(),
                ))
            })
            .collect()
    };
    Ok(mods(current)? == mods(deployed)? && routes(current)? == routes(deployed)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use records::{Rows, Table};

    // @variants: both
    #[test]
    fn absolute_routes_match_retained_sources_but_routing_changes_do_not() -> Result<()> {
        for writable in [false, true] {
            let cache = Path::new("/cache");
            let data = Path::new("/data");
            let id = "00000000-0000-0000-0000-000000000001";
            let hash = "a".repeat(64);
            let root = if writable {
                cache.join(id)
            } else {
                data.join("mele-sources").join(&hash)
            };
            let mut current = vec![Rows {
                table: Table::MelePackages,
                rows: vec![BTreeMap::from([
                    ("mod_id".into(), Value::String(id.into())),
                    ("document".into(), Value::String(serde_json::json!({"version":3,"target":"LE1","writable_cache":writable,"package":{"id":id,"source_sha256":hash,"archive_sha256":null,"manifest_version":"9.1","mod_version":"1.0","enabled":true,"options":[]}}).to_string())),
                ])],
            }, Rows {
                table: Table::Files,
                rows: vec![BTreeMap::from([
                    ("mod_id".into(), Value::String(id.into())),
                    ("game_rel_original".into(), Value::String("BioGame/Test.pcc".into())),
                    ("game_rel_lowercase".into(), Value::String("biogame/test.pcc".into())),
                    ("cache_path".into(), Value::String(root.join("source.pcc").to_string_lossy().into_owned())),
                ])],
            }];
            let mut retained = current.clone();
            retained[1].rows[0].insert(
                "cache_path".into(),
                Value::String("cache/00000000-0000-0000-0000-000000000001/source.pcc".into()),
            );
            normalize_routes(&mut current, cache, data)?;
            assert!(same_mods(&current, &retained)?);
            current[1].rows[0].insert(
                "cache_path".into(),
                Value::String("cache/00000000-0000-0000-0000-000000000001/other.pcc".into()),
            );
            assert!(!same_mods(&current, &retained)?);
            current[1].rows[0].insert(
                "cache_path".into(),
                Value::String("/outside/source.pcc".into()),
            );
            assert!(normalize_routes(&mut current, cache, data).is_err());
            current[1].rows[0].insert(
                "cache_path".into(),
                Value::String(root.to_string_lossy().into_owned()),
            );
            normalize_routes(&mut current, cache, data)?;
            assert_eq!(
                records::text(&current[1].rows[0], "cache_path")?,
                "cache/00000000-0000-0000-0000-000000000001"
            );
        }
        Ok(())
    }

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
