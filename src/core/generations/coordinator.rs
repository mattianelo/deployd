use anyhow::{Context, Result, ensure};

use crate::core::save_manager::SaveSetId;
use crate::models::game::{Game, GameEngine};

use super::catalog::{History, durable};
use super::content::Control;
use super::journal::Journal;
use super::state::{self, Deployment, State};
use super::target::Target;

pub(super) async fn activate(
    history: &History,
    game: &Game,
    journal: &Journal,
    previous: Option<&State>,
    deployment: Option<&Deployment<'_>>,
    saves: &SaveSetId,
    control: Control,
) -> Result<()> {
    control.check()?;
    (control.phase)("Verifying prepared deployment…");
    validate(history, game, journal, previous, deployment, saves).await?;
    let inputs = deployment
        .map(|deployment| deployment.manifest.base_inputs.clone())
        .unwrap_or_default();
    journal
        .verify_prepared(history, game, inputs, control.clone())
        .await?;
    control.check()?;
    journal
        .persist(
            history,
            game,
            if deployment.is_some() {
                "deploy"
            } else {
                "purge"
            },
            deployment
                .map(|deployment| deployment.manifest.objects())
                .unwrap_or_default(),
        )
        .await?;
    let attempt = async {
        control.check()?;
        (control.phase)("Activating files and saves…");
        let applied = journal.apply(history, game, control.clone()).await?;
        control.check()?;
        (control.phase)("Committing deployment…");
        applied
            .commit(history, game, previous, deployment, saves)
            .await
    }
    .await;
    (control.phase)(if attempt.is_ok() {
        "Finishing deployment…"
    } else {
        "Recovering deployment…"
    });
    finish(history, game, journal, attempt).await
}

async fn validate(
    history: &History,
    game: &Game,
    journal: &Journal,
    previous: Option<&State>,
    deployment: Option<&Deployment<'_>>,
    saves: &SaveSetId,
) -> Result<()> {
    ensure!(
        history.game == game.id,
        "Activation belongs to another game"
    );
    ensure!(
        game.engine != GameEngine::MassEffect || journal.mele.is_some(),
        "MELE activation requires its coordinated engine participant"
    );
    history.tracker.ensure_location_ready(&game.id).await?;
    journal.validate_request(game, previous, deployment, saves)?;
    if let Some(snapshot) = deployment.and_then(|deployment| deployment.manifest.mele.clone()) {
        let tracker = history.tracker.clone();
        let game = game.clone();
        let current = journal.mele.as_ref().and_then(|mele| mele.previous.clone());
        history
            .lease
            .participant(async move {
                snapshot
                    .verify_inputs(&tracker, &game, current.as_ref())
                    .await
            })
            .await
            .context("MELE base verification participant stopped")??;
    }
    let mut tx = durable(&history.tracker).await?;
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id=?)")
            .bind(&game.id)
            .fetch_one(&mut *tx)
            .await?;
    ensure!(
        !pending,
        "Finish the pending operation before activating another configuration"
    );
    ensure!(
        state::read(&mut tx, &game.id).await?.as_ref() == previous,
        "Deployed state or live save ownership changed after preparation"
    );
    if let Some(deployment) = deployment {
        state::validate_deployment(&mut tx, history, game, deployment, saves).await?;
    }
    tx.rollback().await?;
    let mut targets = Vec::new();
    for file in history.tracker.get_deployed_files(&game.id).await? {
        targets.push(Target::file(&game.engine, &file.game_rel_original)?);
    }
    if let Some(generation) = previous.and_then(|state| state.generation.as_deref()) {
        targets.extend(
            history
                .outputs(generation)
                .await?
                .into_iter()
                .map(|output| output.target),
        );
    }
    let changes = journal
        .changes
        .iter()
        .map(|change| change.target.resolve(game))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    for target in targets {
        ensure!(
            changes.contains(&journal.physical(&target).resolve(game)?),
            "Prepared activation omits a previously managed target"
        );
    }
    Ok(())
}

pub(super) async fn finish(
    history: &History,
    game: &Game,
    journal: &Journal,
    attempt: Result<()>,
) -> Result<()> {
    let failure = attempt.as_ref().err().map(|error| format!("{error:#}"));
    let committed = journal.decision(history).await.with_context(|| {
        format!(
            "{}; the durable decision could not be read. Recovery is required before continuing",
            failure
                .as_deref()
                .unwrap_or("Activation completion could not be confirmed")
        )
    })?;
    journal
        .recover(history, game, committed)
        .await
        .with_context(|| {
            let decision = if committed {
                "committed; cleanup is blocked"
            } else {
                "did not commit; rollback is blocked"
            };
            match &failure {
                Some(error) => {
                    format!("{error}; activation {decision}. Recovery information was preserved")
                }
                None => format!("Activation {decision}. Recovery information was preserved"),
            }
        })?;
    if let Err(error) = attempt {
        return Err(error).context(if committed {
            "Activation committed and recovery completed; refresh deployment status before continuing"
        } else {
            "Activation failed; the previous managed files and live saves were restored"
        });
    }
    ensure!(
        committed,
        "Activation did not commit; the previous managed files and live saves were restored"
    );
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/generations/activation.rs"]
mod tests;
