use anyhow::{Context, Result, ensure};

use crate::models::game::{Game, GameEngine};
use crate::models::manifest::ModFile;

use super::{History, Journal, Manifest, Node, Output, Table, Target, text};
use crate::core::generations::{content::Control, prepared};

pub(in crate::core::generations) struct Prepared {
    pub(in crate::core::generations) manifest: Manifest,
    pub(in crate::core::generations) journal: Journal,
    pub(in crate::core::generations) files: Vec<ModFile>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::core::generations) reused: Option<String>,
}

pub(in crate::core::generations) async fn prepare(
    history: &History,
    game: &Game,
    mut manifest: Manifest,
    mut journal: Journal,
    reuse_history: bool,
    control: Control,
) -> Result<Prepared> {
    ensure!(
        game.engine == GameEngine::Bethesda && manifest.engine == GameEngine::Bethesda,
        "This configuration preparation belongs to Bethesda"
    );
    ensure!(
        history.game == game.id && manifest.game_id == game.id && journal.game == game.id,
        "Prepared configuration belongs to another game"
    );
    ensure!(
        !journal.has_saves(),
        "Prepare Bethesda configuration before saves"
    );
    ensure!(
        manifest.shared_revision.is_none(),
        "Bethesda cannot use shared launcher history"
    );
    manifest.validate()?;
    control.check()?;
    history.tracker.ensure_location_ready(&game.id).await?;
    let expected = prepared::files(&manifest)?;
    ensure!(
        manifest.outputs == expected,
        "Prepare the complete frozen mod outputs before configuration"
    );
    for output in &expected {
        ensure!(
            journal
                .changes
                .iter()
                .any(|change| change.target == output.target && change.after == node(output)),
            "The activation journal omits a frozen mod output"
        );
    }
    let lineage: Option<(String, String)> = sqlx::query_as(
        "SELECT generation_id,fingerprint FROM generation_drafts WHERE game_id=? AND profile_id=?",
    )
    .bind(&game.id)
    .bind(manifest.profile()?.0)
    .fetch_optional(&history.tracker.pool)
    .await?;
    let fingerprint = manifest.fingerprint()?;
    let retained = if reuse_history {
        lineage.filter(|(_, previous)| fingerprint == *previous)
    } else {
        None
    };
    let reused = if let Some((id, _)) = retained {
        let original = history.load_with_control(&id, control.clone()).await?;
        ensure!(
            original.engine == GameEngine::Bethesda,
            "Restored configuration belongs to another engine"
        );
        let outputs = original
            .outputs
            .into_iter()
            .filter(|output| is_configuration(&output.target))
            .collect::<Vec<_>>();
        complete(history, game, &outputs).await?;
        let plugins = outputs
            .iter()
            .filter(|output| matches!(output.target, Target::PluginControl { .. }))
            .cloned()
            .collect::<Vec<_>>();
        journal
            .extend(
                history,
                game,
                plugins
                    .iter()
                    .map(|output| (output.target.clone(), node(output)))
                    .collect(),
                control.clone(),
            )
            .await?;
        let inis =
            super::prepare_inis(history, game, journal, Some(outputs), control.clone()).await?;
        journal = inis.journal;
        manifest.outputs.extend(plugins);
        manifest.outputs.extend(inis.outputs);
        manifest.base_inputs = original.base_inputs;
        Some(id)
    } else {
        let outputs = super::plugins(history, game, &manifest, control.clone()).await?;
        journal
            .extend(
                history,
                game,
                outputs
                    .iter()
                    .map(|output| (output.target.clone(), node(output)))
                    .collect(),
                control.clone(),
            )
            .await?;
        manifest.outputs.extend(outputs);
        let ini = super::inis(history, game, journal, control.clone()).await?;
        journal = ini.journal;
        manifest.outputs.extend(ini.outputs);
        None
    };
    manifest
        .outputs
        .sort_by(|left, right| left.target.cmp(&right.target));
    complete(history, game, &manifest.outputs).await?;
    manifest.validate()?;
    journal
        .verify_prepared(history, game, manifest.base_inputs.clone(), control)
        .await?;
    let files = deployment_files(history, &manifest)?;
    Ok(Prepared {
        manifest,
        journal,
        files,
        reused,
    })
}

pub(in crate::core::generations) async fn purge(
    history: &History,
    game: &Game,
    mut journal: Journal,
    previous: &Manifest,
    control: Control,
) -> Result<Journal> {
    ensure!(
        game.engine == GameEngine::Bethesda
            && previous.engine == GameEngine::Bethesda
            && history.game == game.id
            && previous.game_id == game.id
            && journal.game == game.id,
        "Configuration purge belongs to another game or engine"
    );
    ensure!(!journal.has_saves(), "Purge cannot switch live saves");
    previous.validate()?;
    let targets = previous
        .outputs
        .iter()
        .filter(|output| is_configuration(&output.target))
        .map(|output| output.target.clone())
        .collect::<Vec<_>>();
    let reading = control.clone();
    let game_copy = game.clone();
    let desired = history
        .lease
        .blocking(move || -> Result<_> {
            targets
                .into_iter()
                .map(|target| {
                    reading.check()?;
                    let node =
                        crate::core::generations::divergence::live(&game_copy, &target, &reading)?;
                    Ok((target, node))
                })
                .collect::<Result<Vec<_>>>()
        })
        .await
        .context("Purge configuration inspection worker stopped")??;
    journal
        .extend(history, game, desired, control.clone())
        .await?;
    ensure!(
        journal
            .changes
            .iter()
            .filter(|change| is_configuration(&change.target))
            .all(|change| change.before == change.after),
        "Configuration changed during purge preparation; prepare again"
    );
    journal
        .verify_prepared(history, game, vec![], control)
        .await?;
    Ok(journal)
}

fn is_configuration(target: &Target) -> bool {
    matches!(
        target,
        Target::PluginControl { .. } | Target::CustomIni { .. }
    )
}

fn node(output: &Output) -> Node {
    match &output.content {
        Some(identity) => Node::File {
            identity: identity.clone(),
            mode: output.mode,
        },
        None => Node::Directory { mode: output.mode },
    }
}

async fn complete(history: &History, game: &Game, outputs: &[Output]) -> Result<()> {
    let game = game.clone();
    let outputs = outputs.to_vec();
    history
        .lease
        .blocking(move || complete_at(&game, &outputs))
        .await
        .context("Configuration validation worker stopped")?
}

fn complete_at(game: &Game, outputs: &[Output]) -> Result<()> {
    let plugins = crate::core::game::plugins_txt_paths(game).len();
    let inis = crate::core::game::custom_ini_paths(game).len();
    ensure!(
        plugins > 0 && inis > 0,
        "Restore the game's Wine-prefix access before preparing configuration"
    );
    for target in (0..plugins)
        .map(|slot| Target::PluginControl { slot })
        .chain((0..inis).map(|slot| Target::CustomIni { slot }))
    {
        ensure!(
            outputs
                .iter()
                .filter(|output| output.target == target
                    && output.content.is_some()
                    && output.mod_id.is_none())
                .count()
                == 1,
            "Retained Bethesda configuration is incomplete; explicitly prepare a new generation"
        );
    }
    for output in outputs
        .iter()
        .filter(|output| is_configuration(&output.target))
    {
        output.target.resolve(game)?;
    }
    Ok(())
}

pub(in crate::core::generations) fn deployment_files(
    history: &History,
    manifest: &Manifest,
) -> Result<Vec<ModFile>> {
    let rows = &manifest
        .records
        .iter()
        .find(|rows| rows.table == Table::Files)
        .context("Frozen file metadata is missing")?
        .rows;
    let mut files = Vec::new();
    for output in manifest
        .outputs
        .iter()
        .filter(|output| output.mod_id.is_some())
    {
        let row = rows
            .iter()
            .find(|row| {
                text(row, "mod_id").ok() == output.mod_id.as_deref()
                    && text(row, "game_rel_original")
                        .ok()
                        .and_then(|path| Target::file(&manifest.engine, path).ok())
                        .as_ref()
                        == Some(&output.target)
            })
            .context("Frozen output has no deployment record")?;
        let cache = text(row, "cache_path")?
            .strip_prefix("cache/")
            .context("Invalid frozen cache anchor")?;
        super::relative(cache)?;
        files.push(ModFile {
            mod_id: text(row, "mod_id")?.into(),
            game_rel_original: text(row, "game_rel_original")?.into(),
            game_rel_lowercase: text(row, "game_rel_lowercase")?.into(),
            cache_path: history.cache.join(cache).to_string_lossy().into_owned(),
        });
    }
    Ok(files)
}
