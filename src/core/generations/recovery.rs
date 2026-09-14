use anyhow::{Context, Result, bail, ensure};

use crate::models::game::Game;

use super::catalog::History;
use super::journal::Journal;

pub(super) async fn recover(history: &History, game: &Game) -> Result<()> {
    recover_journal(history, game).await?;
    let game = game.clone();
    history
        .lease
        .participant(async move {
            crate::core::save_manager::activation::recovery::recover(&game).await
        })
        .await
        .context("Save preparation recovery participant stopped")??;
    history.finish_deletions().await
}

pub(super) async fn recover_journal(history: &History, game: &Game) -> Result<()> {
    ensure!(history.game == game.id, "Recovery targets a different game");
    let pending: Option<(String, i64, String, bool)> = sqlx::query_as(
        "SELECT kind,document_version,document,committed FROM generation_journals WHERE game_id=?",
    )
    .bind(&game.id)
    .fetch_optional(&history.tracker.pool)
    .await?;
    if let Some((kind, version, document, committed)) = pending {
        match (kind.as_str(), version) {
            ("deploy" | "purge", 1..=3) => {
                let journal = Journal::from_record(version, &document)?;
                journal.recover(history, game, committed).await?;
            }
            ("restore", 1) => super::restore::recover(history).await?,
            _ => bail!(
                "The pending {kind} operation requires its compatible recovery handler; records were preserved"
            ),
        }
    }
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM generation_journals WHERE game_id=?")
            .bind(&game.id)
            .fetch_one(&history.tracker.pool)
            .await?;
    ensure!(
        pending == 0,
        "Pending activation still owns preparation data; cleanup was blocked"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use super::*;
    use crate::core::generations::content::Control;
    use crate::core::generations::journal::Node;
    use crate::core::generations::target::Target;

    // @variants: both
    #[tokio::test]
    async fn dispatcher_rolls_back_uncommitted_files_and_preserves_external_changes() -> Result<()>
    {
        for edited in [false, true] {
            let temp = tempfile::tempdir()?;
            let (tracker, game, _) = super::super::tests::snapshot_fixture(temp.path()).await?;
            fs::create_dir_all(game.data_dir())?;
            let live = game.data_dir().join("file.txt");
            fs::write(&live, b"original")?;
            let history = History::open(&tracker, &game.id, temp.path(), true).await?;
            let content = history
                .retain(temp.path().join("winner/file.txt"), Control::default())
                .await?;
            let journal = Journal::prepare(
                &history,
                &game,
                vec![(
                    Target::file(&game.engine, "file.txt")?,
                    Node::File {
                        identity: content,
                        mode: 0o644,
                    },
                )],
                Control::default(),
            )
            .await?;
            journal
                .persist(&history, &game, "deploy", BTreeMap::new())
                .await?;
            journal.apply(&history, &game, Control::default()).await?;
            if edited {
                fs::write(&live, b"external")?;
            }
            assert_eq!(recover_journal(&history, &game).await.is_err(), edited);
            assert_eq!(
                fs::read(&live)?,
                if edited {
                    b"external".as_slice()
                } else {
                    b"original".as_slice()
                }
            );
            let pending: i64 =
                sqlx::query_scalar("SELECT count(*) FROM generation_journals WHERE game_id=?")
                    .bind(&game.id)
                    .fetch_one(&tracker.pool)
                    .await?;
            assert_eq!(pending, i64::from(edited));
        }
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn unsupported_and_inconsistent_journals_remain_pending() -> Result<()> {
        for (kind, version) in [("shared", 1), ("deploy", 4), ("deploy", 2)] {
            let temp = tempfile::tempdir()?;
            let (tracker, game, _) = super::super::tests::snapshot_fixture(temp.path()).await?;
            let history = History::open(&tracker, &game.id, temp.path(), true).await?;
            let journal = Journal::prepare(&history, &game, vec![], Control::default()).await?;
            let document = serde_json::to_string(&journal)?;
            sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES (?,?,?,?,?)")
                .bind(&journal.id).bind(&game.id).bind(kind).bind(version).bind(&document)
                .execute(&tracker.pool).await?;
            assert!(recover(&history, &game).await.is_err());
            let preserved: String =
                sqlx::query_scalar("SELECT document FROM generation_journals WHERE id=?")
                    .bind(&journal.id)
                    .fetch_one(&tracker.pool)
                    .await?;
            assert_eq!(preserved, document);
        }
        Ok(())
    }
}
