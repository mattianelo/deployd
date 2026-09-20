use anyhow::{Context, Result, ensure};

use crate::core::game::mass_effect::generations::Prepared;
use crate::models::game::{Game, GameEngine};

use super::catalog::History;
use super::content::{Control, Identity};
use super::journal::{Journal, Node};
use super::manifest::{Manifest, Output};
use super::target::Target;

pub(super) async fn retain(
    history: &History,
    game: &Game,
    manifest: &mut Manifest,
    prepared: Prepared,
    shared: Journal,
    control: Control,
) -> Result<Journal> {
    ensure!(
        game.engine == GameEngine::MassEffect
            && manifest.engine == game.engine
            && manifest.game_id == game.id,
        "MELE preparation belongs to another game"
    );
    ensure!(
        manifest.profile()?.0 == prepared.journal.desired.profile,
        "MELE preparation belongs to another profile"
    );
    let attempt = async {
        let mut retained = manifest.clone();
        retained.version = 2;
        retained.mele = Some(prepared.snapshot.clone());
        let profile = retained.profile()?.0.to_owned();
        let recipe_rows = retained
            .records
            .iter_mut()
            .find(|rows| rows.table == super::records::Table::MeleRecipes)
            .context("Missing MELE recipe metadata")?;
        recipe_rows.rows = vec![std::collections::BTreeMap::from([
            ("game_id".into(), serde_json::Value::String(game.id.clone())),
            ("profile_id".into(), serde_json::Value::String(profile)),
            (
                "document".into(),
                serde_json::Value::String(serde_json::to_string(&prepared.snapshot.recipe)?),
            ),
        ])];
        retained.outputs.clear();
        for file in &prepared.snapshot.files {
            control.check()?;
            let identity = history
                .retain(prepared.source(&file.relative), control.clone())
                .await?;
            ensure!(
                identity.sha256 == file.sha256 && identity.size == file.size,
                "Prepared MELE output changed before retention"
            );
            retained.outputs.push(Output {
                target: Target::MassEffect {
                    path: file.relative.clone(),
                },
                content: Some(identity),
                mode: 0o644,
                mod_id: None,
            });
        }
        let mut journal =
            activation(history, game, &prepared, &retained.outputs, control.clone()).await?;
        journal.attach_game_shared(history, game, shared).await?;
        let dependency = super::shared::identity(journal.dependency.as_ref())?;
        retained.shared_revision = dependency;
        journal.validate(game)?;
        retained.validate()?;
        Ok::<_, anyhow::Error>((retained, journal))
    }
    .await;
    prepared
        .discard()
        .await
        .context("MELE generation staging cleanup failed")?;
    let (retained, journal) = attempt?;
    *manifest = retained;
    Ok(journal)
}

pub(super) async fn reuse(
    history: &History,
    game: &Game,
    manifest: &mut Manifest,
    data: std::path::PathBuf,
    control: Control,
) -> Result<Option<Journal>> {
    ensure!(
        game.engine == GameEngine::MassEffect,
        "Retained MELE outputs belong to another engine"
    );
    let lineage: Option<(String, String)> = sqlx::query_as(
        "SELECT generation_id,fingerprint FROM generation_drafts WHERE game_id=? AND profile_id=?",
    )
    .bind(&game.id)
    .bind(manifest.profile()?.0)
    .fetch_optional(&history.tracker.pool)
    .await?;
    let Some((generation, fingerprint)) = lineage else {
        return Ok(None);
    };
    if fingerprint != manifest.fingerprint()? {
        return Ok(None);
    }
    let original = history
        .load_with_control(&generation, control.clone())
        .await?;
    let mut snapshot = original
        .mele
        .context("Historical MELE generation has no retained outputs")?;
    let recipe = manifest
        .records
        .iter()
        .find(|rows| rows.table == super::records::Table::MeleRecipes)
        .and_then(|rows| rows.rows.first())
        .context("Restored MELE recipe is missing")?;
    snapshot.recipe = serde_json::from_str(super::records::text(recipe, "document")?)?;
    snapshot.validate(&game.id)?;
    let shared = super::shared::prepare(
        history,
        game,
        crate::core::game::mass_effect::family::generations::Action::Support(
            snapshot.recipe.clone(),
        ),
        data.clone(),
        control.clone(),
    )
    .await?;
    let store = history.store.clone();
    let files = snapshot.files.clone();
    let cache = history.cache.clone();
    let copying = control.clone();
    let source = history
        .lease
        .blocking(move || -> Result<_> {
            let directory = tempfile::Builder::new()
                .prefix(".mele-history-")
                .tempdir_in(cache)?;
            for file in files {
                copying.check()?;
                let path = directory.path().join(file.relative);
                std::fs::create_dir_all(path.parent().context("MELE output has no parent")?)?;
                store.materialize(
                    &Identity {
                        size: file.size,
                        sha256: file.sha256,
                    },
                    &path,
                    0o600,
                    &copying,
                )?;
            }
            Ok(directory)
        })
        .await
        .context("Retained MELE materialization worker stopped")??;
    let destination = crate::core::game::mass_effect::recipe::Destination {
        game: game.clone(),
        profile: manifest.profile()?.0.into(),
        backend: None,
        previous: history
            .tracker
            .mele_deployment(&game.id)
            .await?
            .map(|state| state.generation),
        repair_components: false,
    };
    let tracker = history.tracker.clone();
    let preparing = control.clone();
    let prepared = history
        .lease
        .participant(async move {
            crate::core::game::mass_effect::generations::retained(
                tracker,
                destination,
                snapshot,
                source,
                data.clone(),
                preparing,
            )
            .await
        })
        .await
        .context("Retained MELE preparation participant stopped")??;
    let mut restored = manifest.clone();
    restored.shared_revision = original.shared_revision;
    let journal = retain(history, game, &mut restored, prepared, shared, control).await?;
    *manifest = restored;
    Ok(Some(journal))
}

mod frozen;

pub(super) async fn prepare(
    history: &History,
    destination: crate::core::game::mass_effect::recipe::Destination,
    manifest: &mut Manifest,
    recipe: crate::core::game::mass_effect::recipe::Recipe,
    data: std::path::PathBuf,
    control: Control,
) -> Result<Journal> {
    let game = destination.game.clone();
    ensure!(
        manifest.game_id == game.id && manifest.profile()?.0 == destination.profile,
        "MELE preparation differs from its frozen profile"
    );
    let (sources, mut frozen, recipe) =
        frozen::sources(history, manifest, recipe, control.clone()).await?;
    let shared = super::shared::prepare(
        history,
        &game,
        crate::core::game::mass_effect::family::generations::Action::Support(recipe.clone()),
        data.clone(),
        control.clone(),
    )
    .await?;
    let tracker = history.tracker.clone();
    let preparing = control.clone();
    let prepared = history
        .lease
        .participant(async move {
            crate::core::game::mass_effect::generations::fresh(
                tracker,
                destination,
                recipe,
                sources,
                data,
                preparing,
            )
            .await
        })
        .await
        .context("Frozen MELE preparation participant stopped")??;
    let journal = retain(history, &game, &mut frozen, prepared, shared, control).await?;
    *manifest = frozen;
    Ok(journal)
}

async fn activation(
    history: &History,
    game: &Game,
    prepared: &Prepared,
    outputs: &[Output],
    control: Control,
) -> Result<Journal> {
    let mut desired = std::collections::BTreeMap::new();
    for (path, _, after) in prepared.journal.generation_targets() {
        let node = if let Some((size, sha256)) = after {
            let expected = Identity { size, sha256 };
            ensure!(
                history
                    .retain(prepared.source(&path), control.clone())
                    .await?
                    == expected,
                "Prepared MELE restoration output changed"
            );
            Node::File {
                identity: expected,
                mode: 0o644,
            }
        } else {
            Node::Absent
        };
        desired.insert(Target::MassEffect { path }, node);
    }
    for output in outputs {
        desired.insert(
            output.target.clone(),
            Node::File {
                identity: output.content.clone().context("Missing MELE output")?,
                mode: output.mode,
            },
        );
    }
    if let Some(previous) = &prepared.journal.previous {
        for file in &previous.files {
            let target = Target::MassEffect {
                path: file.relative.clone(),
            };
            if let std::collections::btree_map::Entry::Vacant(entry) = desired.entry(target) {
                let expected = Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                };
                ensure!(
                    history
                        .retain(game.path.join(&file.relative), control.clone())
                        .await?
                        == expected,
                    "Previously managed MELE file changed during preparation"
                );
                entry.insert(Node::File {
                    identity: expected,
                    mode: 0o644,
                });
            }
        }
    }
    let root = game.path.clone();
    let removals = prepared.snapshot.removals.dlc.clone();
    let outputs = prepared.snapshot.files.clone();
    let scanning = control.clone();
    let directories = history
        .lease
        .blocking(move || -> Result<Vec<String>> {
            let mut directories = Vec::new();
            for scope in removals {
                let dlc = root.join("BioGame/DLC").join(scope);
                if !dlc.try_exists()? {
                    continue;
                }
                for entry in walkdir::WalkDir::new(dlc).follow_links(false) {
                    scanning.check()?;
                    let entry = entry?;
                    ensure!(
                        entry.file_type().is_dir() || entry.file_type().is_file(),
                        "Unexpected link in obsolete MELE DLC"
                    );
                    if entry.file_type().is_dir() {
                        let path = entry
                            .path()
                            .strip_prefix(&root)?
                            .to_str()
                            .context("Invalid obsolete DLC path")?;
                        if !outputs
                            .iter()
                            .any(|file| file.relative.starts_with(&format!("{path}/")))
                        {
                            directories.push(path.to_owned());
                        }
                    }
                }
            }
            Ok(directories)
        })
        .await
        .context("Obsolete MELE directory preparation stopped")??;
    for path in directories {
        desired.insert(Target::MassEffect { path }, Node::Absent);
    }
    let mut journal = Journal::prepare(
        history,
        game,
        desired.into_iter().collect(),
        control.clone(),
    )
    .await?;
    journal.attach_mele(game, prepared.journal.clone())?;
    Ok(journal)
}

pub(super) async fn purge(
    history: &History,
    game: &Game,
    data: std::path::PathBuf,
    control: Control,
) -> Result<Journal> {
    let target = crate::core::game::mass_effect::library::target(game)?;
    let shared = super::shared::prepare(
        history,
        game,
        crate::core::game::mass_effect::family::generations::Action::Support(
            crate::core::game::mass_effect::recipe::Recipe {
                version: 1,
                target,
                backend_version: 1,
                helper_version: None,
                language: "INT".into(),
                packages: Vec::new(),
                launcher: Vec::new(),
                components: Vec::new(),
            },
        ),
        data.clone(),
        control.clone(),
    )
    .await?;
    let cache = history.cache.clone();
    let source = history
        .lease
        .blocking(move || {
            tempfile::Builder::new()
                .prefix(".mele-purge-")
                .tempdir_in(cache)
        })
        .await
        .context("MELE purge staging worker stopped")??;
    let tracker = history.tracker.clone();
    let purging = game.clone();
    let preparing = control.clone();
    let prepared = history
        .lease
        .participant(async move {
            crate::core::game::mass_effect::generations::purge(
                tracker,
                purging,
                data.clone(),
                source,
                preparing,
            )
            .await
        })
        .await
        .context("MELE purge participant stopped")??;
    let attempt = async {
        let mut journal = activation(history, game, &prepared, &[], control.clone()).await?;
        journal.attach_game_shared(history, game, shared).await?;
        journal.validate(game)?;
        Ok(journal)
    }
    .await;
    prepared
        .discard()
        .await
        .context("MELE purge staging cleanup failed")?;
    attempt
}
