#![cfg_attr(not(test), allow(dead_code))]

use super::*;

pub(in crate::core::game::mass_effect) async fn stage(
    tracker: &Tracker,
    game: &Game,
    plan: &super::super::family::Plan,
    data: &Path,
    control: &Control,
) -> Result<Journal> {
    tracker.ensure_no_mele_journal(&game.id).await?;
    let baseline = tracker
        .load_mele_baseline(&game.id)
        .await?
        .context("Finish MELE setup before changing launcher mods")?;
    let previous = tracker.mele_deployment(&game.id).await?;
    let mut desired = if let Some(previous) = &previous {
        previous.clone()
    } else {
        State {
            version: 1,
            generation: String::new(),
            profile: tracker
                .get_active_profile(&game.id)
                .await?
                .context("Select a game profile before managing launcher mods")?
                .id,
            files: Vec::new(),
            recipe: None,
            removals: Default::default(),
        }
    };
    desired.generation = Uuid::new_v4().to_string();
    let journal = Journal {
        family_only: true,
        version: 6,
        id: desired.generation.clone(),
        game_id: game.id.clone(),
        baseline: baseline.sha256.clone(),
        previous,
        desired,
        family: Some(plan.change.clone()),
        missing_components: Vec::new(),
        operations: Vec::new(),
        directories: Vec::new(),
    };
    journal.validate(game, &baseline)?;
    plan.preserve(tracker, data, control.clone()).await?;
    let stage = plan.change.storage(data, &journal.id);
    let staged = {
        let plan = plan.clone();
        let data = data.to_path_buf();
        let id = journal.id.clone();
        let control = control.clone();
        tokio::task::spawn_blocking(move || plan.stage(&data, &id, &control)).await?
    };
    if let Err(error) = staged {
        plan.change.cleanup(&stage)?;
        return Err(error);
    }
    if let Err(error) = control.check() {
        plan.change.cleanup(&stage)?;
        return Err(error);
    }
    Ok(journal)
}

pub(in crate::core::game::mass_effect) async fn publish(
    tracker: &Tracker,
    game: &Game,
    plan: super::super::family::Plan,
    data: &Path,
    control: &Control,
) -> Result<()> {
    let journal = stage(tracker, game, &plan, data, control).await?;
    let stage = plan.change.storage(data, &journal.id);
    if let Err(error) = tracker.begin_mele_journal(&journal).await {
        if tracker.mele_journal(&game.id).await?.is_some() {
            recover_locked(tracker, game, data).await?;
        } else {
            plan.change.cleanup(&stage)?;
        }
        return Err(error);
    }
    let result = {
        let control = control.clone();
        let stage = stage.clone();
        let plan = plan.clone();
        tokio::task::spawn_blocking(move || {
            plan.change.apply(&plan.root, &stage, &control)?;
            control.check()
        })
        .await?
    };
    let result = match result {
        Ok(()) => tracker.commit_mele_journal(&journal).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        let (_, committed) = tracker
            .mele_journal(&game.id)
            .await?
            .context("Launcher journal disappeared; reconciliation is required")?;
        recover_locked(tracker, game, data).await.with_context(|| {
            format!("Launcher changes failed ({error:#}); recovery must finish before continuing")
        })?;
        if !committed {
            return Err(error);
        }
    } else {
        recover_locked(tracker, game, data).await?;
    }
    Ok(())
}
