use sqlx::{Sqlite, Transaction};

use super::*;

pub(crate) type GenerationTarget = (String, Option<(u64, String)>, Option<(u64, String)>);

impl Journal {
    pub(crate) fn generation_targets(&self) -> Vec<GenerationTarget> {
        self.operations
            .iter()
            .map(|operation| {
                (
                    operation.path.clone(),
                    operation
                        .before
                        .as_ref()
                        .map(|id| (id.size, id.sha256.clone())),
                    operation
                        .after
                        .as_ref()
                        .map(|id| (id.size, id.sha256.clone())),
                )
            })
            .collect()
    }

    pub(crate) fn generation_shape(&self, game: &Game) -> Result<()> {
        target(game)?;
        ensure!(
            self.game_id == game.id && !self.family_only && self.family.is_none(),
            "Game generations cannot apply shared launcher changes"
        );
        ensure!(
            Uuid::parse_str(&self.id).is_ok() && self.desired.generation == self.id,
            "Invalid MELE generation participant identity"
        );
        Ok(())
    }

    pub(crate) async fn generation_check(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        committed: bool,
    ) -> Result<()> {
        let current: Option<String> =
            sqlx::query_scalar("SELECT document FROM mele_deployments WHERE game_id=?")
                .bind(&self.game_id)
                .fetch_optional(&mut **tx)
                .await?;
        let current: Option<State> = current
            .map(|value| serde_json::from_str(&value))
            .transpose()?;
        ensure!(
            current
                == if committed {
                    Some(self.desired.clone())
                } else {
                    self.previous.clone()
                },
            "MELE deployment state differs from its generation recovery intent"
        );
        let baseline: String =
            sqlx::query_scalar("SELECT inventory_sha256 FROM mele_baselines WHERE game_id=?")
                .bind(&self.game_id)
                .fetch_one(&mut **tx)
                .await?;
        ensure!(
            baseline == self.baseline,
            "MELE restoration baseline changed; recovery information was preserved"
        );
        let legacy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mele_journals WHERE game_id=? OR json_extract(document, '$.family') IS NOT NULL)").bind(&self.game_id).fetch_one(&mut **tx).await?;
        ensure!(
            !legacy,
            "Finish legacy MELE recovery before generation activation"
        );
        Ok(())
    }

    pub(crate) async fn generation_commit(&self, tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
        self.generation_check(tx, false).await?;
        sqlx::query("INSERT INTO mele_deployments(game_id,document) VALUES (?,?) ON CONFLICT(game_id) DO UPDATE SET document=excluded.document").bind(&self.game_id).bind(serde_json::to_string(&self.desired)?).execute(&mut **tx).await?;
        if let Some(recipe) = &self.desired.recipe {
            sqlx::query("INSERT INTO mele_recipes(game_id,profile_id,document) VALUES (?,?,?) ON CONFLICT(game_id,profile_id) DO UPDATE SET document=excluded.document").bind(&self.game_id).bind(&self.desired.profile).bind(serde_json::to_string(recipe)?).execute(&mut **tx).await?;
        }
        Ok(())
    }

    pub(crate) async fn generation_verify(
        &self,
        tracker: &Tracker,
        game: &Game,
        applied: bool,
    ) -> Result<()> {
        self.generation_shape(game)?;
        let baseline = tracker
            .load_mele_baseline(&game.id)
            .await?
            .context("MELE generation requires its restoration baseline")?;
        self.validate(game, &baseline)?;
        let journal = self.clone();
        let game = game.clone();
        tokio::task::spawn_blocking(move || {
            let control = Control::recovery();
            let state = if applied {
                Some(&journal.desired)
            } else {
                journal.previous.as_ref()
            };
            for operation in &journal.operations {
                files::verify(
                    &game.path,
                    &operation.path,
                    if applied {
                        operation.after.as_ref()
                    } else {
                        operation.before.as_ref()
                    },
                    &control,
                )?;
            }
            if let Some(state) = state {
                for file in &state.files {
                    if !applied && journal.missing_components.contains(&file.relative) {
                        continue;
                    }
                    files::verify(
                        &game.path,
                        &file.relative,
                        Some(&Identity {
                            size: file.size,
                            sha256: file.sha256.clone(),
                        }),
                        &control,
                    )?;
                }
            }
            super::super::removal::verify(
                &game.path,
                &baseline,
                state,
                &super::super::removal::scopes(journal.previous.as_ref(), &journal.desired),
                &control,
            )
        })
        .await
        .context("MELE generation verification worker stopped")?
    }
}

impl Journal {
    pub(crate) async fn discard_generation_stage(&self, data: PathBuf) -> Result<()> {
        ensure!(
            !self.family_only && self.family.is_none(),
            "Shared preparation needs its own cleanup"
        );
        discard(&storage(&data, &self.game_id, &self.id), self, &data).await
    }
}

impl Journal {
    pub(crate) async fn generation_validate(&self, tracker: &Tracker, game: &Game) -> Result<()> {
        self.generation_shape(game)?;
        let baseline = tracker
            .load_mele_baseline(&game.id)
            .await?
            .context("MELE generation requires its restoration baseline")?;
        self.validate(game, &baseline)
    }
}

impl Journal {
    pub(crate) fn discard_abandoned_generation_stage(&self, data: &Path) -> Result<()> {
        ensure!(
            !self.family_only && self.family.is_none(),
            "Shared preparation needs its own cleanup"
        );
        files::cleanup(&storage(data, &self.game_id, &self.id), self)
    }
}
