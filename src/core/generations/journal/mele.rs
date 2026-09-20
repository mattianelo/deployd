use super::*;
use crate::core::game::mass_effect::journal::Journal as Mele;
use crate::models::game::GameEngine;

impl Journal {
    pub(in crate::core::generations) fn attach_mele(
        &mut self,
        game: &Game,
        participant: Mele,
    ) -> Result<()> {
        ensure!(
            self.mele.is_none(),
            "Activation already has a MELE participant"
        );
        let mut combined = self.clone();
        combined.version = 4;
        combined.mele = Some(participant);
        combined.validate(game)?;
        *self = combined;
        Ok(())
    }

    pub(super) fn validate_mele(&self, game: &Game) -> Result<()> {
        let Some(mele) = &self.mele else {
            return Ok(());
        };
        ensure!(
            self.version >= 4 && game.engine == GameEngine::MassEffect,
            "MELE activation requires its versioned engine journal"
        );
        mele.generation_shape(game)?;
        let targets = mele.generation_targets();
        let matches = |node: &Node, expected: &Option<(u64, String)>| match (node, expected) {
            (Node::Absent, None) => true,
            (Node::File { identity, .. }, Some((size, hash))) => {
                identity.size == *size && identity.sha256 == *hash
            }
            _ => false,
        };
        for (path, before, after) in &targets {
            ensure!(
                self.changes.iter().any(|change| change.target
                    == (Target::MassEffect { path: path.clone() })
                    && matches(&change.before, before)
                    && matches(&change.after, after)),
                "Activation omits or alters a validated MELE operation"
            );
        }
        for change in &self.changes {
            let path = match &change.target {
                Target::MassEffect { path } => path,
                Target::MeleLauncher { .. } if self.shared.is_some() => continue,
                _ => anyhow::bail!("MELE activation cannot reinterpret another engine anchor"),
            };
            if matches!(
                (&change.before, &change.after),
                (Node::Directory { .. }, Node::Absent)
            ) {
                ensure!(
                    mele.desired
                        .removals
                        .dlc
                        .iter()
                        .any(|name| path == &format!("BioGame/DLC/{name}")
                            || path.starts_with(&format!("BioGame/DLC/{name}/")))
                        && !mele
                            .desired
                            .files
                            .iter()
                            .any(|file| file.relative.starts_with(&format!("{path}/"))),
                    "MELE directory removal escapes its obsolete DLC scope"
                );
                continue;
            }
            if !targets.iter().any(|(target, _, _)| target == path) {
                ensure!(
                    mele.desired
                        .files
                        .iter()
                        .chain(mele.previous.iter().flat_map(|state| &state.files))
                        .any(|file| file.relative == *path
                            && matches(&change.before, &Some((file.size, file.sha256.clone())))
                            && matches(&change.after, &Some((file.size, file.sha256.clone())))),
                    "Activation includes an unvalidated MELE target"
                );
            }
        }
        Ok(())
    }

    pub(super) fn validate_mele_request(
        &self,
        deployment: Option<&super::super::state::Deployment<'_>>,
    ) -> Result<()> {
        let Some(mele) = &self.mele else {
            return Ok(());
        };
        if let Some(deployment) = deployment {
            let snapshot = deployment
                .manifest
                .mele
                .as_ref()
                .context("MELE activation requires its retained engine snapshot")?;
            ensure!(
                mele.desired.profile == deployment.profile
                    && mele.desired.recipe.as_ref() == Some(&snapshot.recipe)
                    && mele.desired.files == snapshot.files
                    && mele.desired.removals == snapshot.removals,
                "MELE activation differs from its retained generation"
            );
        } else {
            ensure!(
                mele.desired.recipe.is_none()
                    && mele.desired.files.is_empty()
                    && mele.desired.removals.is_empty(),
                "MELE purge must restore its complete managed configuration"
            );
        }
        Ok(())
    }

    pub(super) async fn verify_mele(
        &self,
        history: &History,
        game: &Game,
        applied: bool,
    ) -> Result<()> {
        if let Some(mele) = &self.mele {
            let mele = mele.clone();
            let tracker = history.tracker.clone();
            let game = game.clone();
            history
                .lease
                .participant(async move { mele.generation_verify(&tracker, &game, applied).await })
                .await
                .context("MELE generation participant stopped")??;
        }
        Ok(())
    }
}

pub(in crate::core::generations) async fn verify_inputs(
    history: &History,
    game: &Game,
    deployment: Option<&super::super::state::Deployment<'_>>,
    current: Option<&crate::core::game::mass_effect::journal::State>,
) -> Result<()> {
    if let Some(snapshot) = deployment.and_then(|deployment| deployment.manifest.mele.clone()) {
        let tracker = history.tracker.clone();
        let game = game.clone();
        let current = current.cloned();
        history
            .lease
            .participant(async move {
                snapshot
                    .verify_inputs(&tracker, &game, current.as_ref())
                    .await
            })
            .await
            .context("MELE vanilla verification participant stopped")??;
    }
    Ok(())
}
