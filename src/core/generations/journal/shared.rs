#![allow(
    dead_code,
    reason = "legacy shared activation journals remain recoverable after removing standalone launcher actions"
)]

use super::*;

impl Journal {
    pub(in crate::core::generations) fn attach_shared(
        &mut self,
        game: &Game,
        intent: super::super::shared::Intent,
    ) -> Result<()> {
        ensure!(
            self.shared.is_none()
                && self.mele.is_none()
                && self.saves.is_none()
                && self.dependency.is_none(),
            "Shared Apply cannot contain game or save participants"
        );
        let mut combined = self.clone();
        combined.version = 4;
        combined.shared = Some(intent);
        combined.validate(game)?;
        *self = combined;
        Ok(())
    }

    pub(super) fn validate_shared(&self, game: &Game) -> Result<()> {
        let Some(intent) = &self.shared else {
            return Ok(());
        };
        let combined = self.mele.is_some();
        ensure!(
            if combined {
                self.version >= 6
                    && self
                        .dependency
                        .as_ref()
                        .is_some_and(|dependency| dependency == &intent.revision)
            } else {
                self.version >= 4 && self.saves.is_none() && self.dependency.is_none()
            },
            "Unsupported shared activation journal"
        );
        intent.validate(game)?;
        let files = intent.change.generation_files();
        let matches = |node: &Node, id: &Option<Identity>| match (node, id) {
            (Node::Absent, None) => true,
            (Node::File { identity, .. }, Some(id)) => identity == id,
            _ => false,
        };
        for (path, (before, after)) in &files {
            if before.is_none() && after.is_none() {
                continue;
            }
            ensure!(
                self.changes.iter().any(|change| change.target
                    == (Target::MeleLauncher { path: path.clone() })
                    && matches(&change.before, before)
                    && matches(&change.after, after)),
                "Shared journal omits or changes a validated launcher target"
            );
        }
        ensure!(
            self.changes.iter().all(|change| {
                matches!(&change.target, Target::MeleLauncher { path } if files.get(path).is_some_and(|(before, after)| matches(&change.before, before) && matches(&change.after, after)))
                    || combined && matches!(change.target, Target::MassEffect { .. })
            }),
            "Shared Apply cannot modify game files or foreign anchors"
        );
        Ok(())
    }

    pub(in crate::core::generations) async fn attach_game_shared(
        &mut self,
        history: &History,
        game: &Game,
        shared: Self,
    ) -> Result<()> {
        self.validate(game)?;
        shared.validate(game)?;
        let intent = shared
            .shared
            .context("Missing prepared shared launcher participant")?;
        ensure!(
            self.mele.is_some()
                && self.shared.is_none()
                && self.dependency.is_none()
                && shared.mele.is_none()
                && shared.dependency.is_none()
                && shared.saves.is_none()
                && shared.cache_sources.is_empty()
                && shared.links.is_empty(),
            "Shared launcher preparation cannot join this game activation"
        );
        let mut combined = self.clone();
        combined.version = 6;
        combined.changes.extend(shared.changes);
        combined.directories.clear();
        combined.dependency = Some(intent.revision.clone());
        combined.shared = Some(intent);
        let validating_game = game.clone();
        combined = history
            .lease
            .blocking(move || -> Result<Self> {
                combined.capture_directories(&validating_game)?;
                combined.validate(&validating_game)?;
                Ok(combined)
            })
            .await
            .context("Combined launcher activation validation stopped")??;
        *self = combined;
        Ok(())
    }

    pub(super) async fn verify_shared(
        &self,
        history: &History,
        game: &Game,
        applied: bool,
    ) -> Result<()> {
        if let Some(intent) = &self.shared {
            intent.verify(history, game, applied).await?;
        }
        Ok(())
    }
}

impl Applied {
    pub(in crate::core::generations) async fn commit_shared(
        self,
        history: &History,
        game: &Game,
    ) -> Result<()> {
        ensure!(
            self.game == game.id && self.game == history.game,
            "Shared activation belongs to another game"
        );
        let journal: Journal = serde_json::from_str(&self.document)?;
        journal.validate(game)?;
        journal.check_record(history, false).await?;
        let intent = journal
            .shared
            .clone()
            .context("Missing shared activation participant")?;
        journal.verify_shared(history, game, true).await?;
        let copy = game.clone();
        history
            .lease
            .blocking(move || -> Result<()> {
                journal.verify_directories(&copy, true)?;
                for change in &journal.changes {
                    let path = change.target.resolve(&copy)?;
                    layout::accessible(&copy, &change.target, &path, false)?;
                    ensure!(
                        inspect(&path, &Control::default())? == change.after,
                        "Launcher changed before commit; recovery information was preserved"
                    );
                }
                Ok(())
            })
            .await
            .context("Shared final verification stopped")??;
        let mut tx = durable(&history.tracker).await?;
        let kind: String =
            sqlx::query_scalar("SELECT kind FROM generation_journals WHERE id=? AND game_id=?")
                .bind(&self.id)
                .bind(&self.game)
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            kind == "shared",
            "Shared decision targets a different operation"
        );
        intent.publish(&mut tx, game).await?;
        self.decide(&mut tx).await?;
        tx.commit()
            .await
            .context("Shared commit outcome requires durable recovery")
    }
}

impl Journal {
    pub(super) async fn verify_dependency(&self, history: &History, game: &Game) -> Result<()> {
        if self.shared.is_some() {
            return Ok(());
        }
        if let Some(mele) = &self.mele {
            super::super::shared::verify_dependency(
                history,
                game,
                &mele.desired,
                self.dependency.as_ref(),
            )
            .await?;
        }
        Ok(())
    }
}
