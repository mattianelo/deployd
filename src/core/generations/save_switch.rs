use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::core::save_manager::{SaveSetId, activation};
use crate::core::tracker::Tracker;
use crate::models::game::Game;

use super::catalog::durable;
use super::state::{self, State};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    id: String,
    previous: State,
    transition: activation::Transition,
}

pub(super) async fn switch(
    tracker: &Tracker,
    members: &[(Game, SaveSetId, SaveSetId)],
    cap: u64,
) -> Result<()> {
    let mut prepared = Vec::new();
    let result = async {
        for (game, source, target) in members {
            if source == target { continue; }
            let mut tx = durable(tracker).await?;
            let previous = state::read(&mut tx, &game.id).await?.context("Missing live save ownership")?;
            ensure!(&previous.saves == source, "Live save ownership changed before sync");
            tx.rollback().await?;
            let id = uuid::Uuid::new_v4().to_string();
            let transition = activation::prepare_sync(&id, game, source, target, cap).await?;
            prepared.push((game.clone(), Intent { id, previous, transition }));
        }
        let mut tx = durable(tracker).await?;
        for (game, intent) in &prepared {
            ensure!(state::read(&mut tx, &game.id).await?.as_ref() == Some(&intent.previous), "Live save ownership changed during sync");
            sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES (?,?,'restore',2,?)")
                .bind(&intent.id).bind(&game.id).bind(serde_json::to_string(intent)?).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        for (game, intent) in &prepared {
            intent.transition.adopt_preparation(game).await?;
            intent.transition.apply(game).await?;
        }
        let mut tx = durable(tracker).await?;
        for (game, intent) in &prepared {
            ensure!(state::read(&mut tx, &game.id).await?.as_ref() == Some(&intent.previous), "Live save ownership changed during sync");
            intent.transition.verify(game).await?;
            let target = &intent.transition.target;
            sqlx::query("UPDATE generation_game_state SET live_save_profile_id=?,live_save_mode=? WHERE game_id=?")
                .bind(target.profile_id()).bind(if target.profile_id().is_some() { "profile" } else { "global" }).bind(&game.id).execute(&mut *tx).await?;
            if let Some(profile) = target.profile_id() {
                sqlx::query("UPDATE generation_drafts SET seed_live_saves=0 WHERE game_id=? AND profile_id=?")
                    .bind(&game.id).bind(profile).execute(&mut *tx).await?;
            }
            let changed = sqlx::query("UPDATE generation_journals SET committed=1 WHERE id=? AND game_id=? AND committed=0")
                .bind(&intent.id).bind(&game.id).execute(&mut *tx).await?;
            ensure!(changed.rows_affected() == 1, "Save sync recovery record changed");
        }
        tx.commit().await.context("Save sync commitment failed; recovery must resolve its durable decision")
    }.await;
    for (game, intent) in &prepared {
        recover(tracker, game)
            .await
            .with_context(|| match &result {
                Ok(()) => "Saves switched; finish pending recovery before continuing".to_owned(),
                Err(error) => format!("{error:#}; save rollback needs recovery"),
            })?;
        activation::discard_preparation(game, &intent.id).await?;
    }
    result
}

pub(super) async fn recover(tracker: &Tracker, game: &Game) -> Result<()> {
    let pending: Option<(String, String, bool)> = sqlx::query_as("SELECT id,document,committed FROM generation_journals WHERE game_id=? AND kind='restore' AND document_version=2")
        .bind(&game.id).fetch_optional(&tracker.pool).await?;
    let Some((id, document, committed)) = pending else {
        return Ok(());
    };
    let intent: Intent = serde_json::from_str(&document)?;
    ensure!(
        intent.id == id && intent.previous.saves == intent.transition.source,
        "Invalid save sync recovery intent"
    );
    let checked = game.clone();
    let transition = intent.transition.clone();
    tokio::task::spawn_blocking(move || transition.validate(&id, &checked)).await??;
    let mut tx = durable(tracker).await?;
    let mut expected = intent.previous.clone();
    if committed {
        expected.saves = intent.transition.target.clone();
    }
    ensure!(
        state::read(&mut tx, &game.id).await? == Some(expected),
        "Save ownership differs from its recovery decision; saves were preserved"
    );
    tx.rollback().await?;
    intent.transition.adopt_preparation(game).await?;
    intent.transition.recover(game, committed).await?;
    let mut tx = durable(tracker).await?;
    let removed = sqlx::query(
        "DELETE FROM generation_journals WHERE id=? AND game_id=? AND document=? AND committed=?",
    )
    .bind(&intent.id)
    .bind(&game.id)
    .bind(document)
    .bind(committed)
    .execute(&mut *tx)
    .await?;
    ensure!(
        removed.rows_affected() == 1,
        "Save sync recovery record changed"
    );
    tx.commit().await?;
    Ok(())
}
