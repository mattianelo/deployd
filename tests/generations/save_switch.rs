use super::*;

// @variants: both
#[test]
fn save_only_switch_starts_empty_and_preserves_mod_deployment() -> Result<()> {
    isolated(
        "switching::save_only_switch_starts_empty_and_preserves_mod_deployment",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            let target = SaveSetId::Profile {
                game_id: fixture.game.id.clone(),
                profile_id: fixture.profile.clone(),
            };
            super::super::super::save_switch::switch(
                &fixture.history.tracker,
                &[(fixture.game.clone(), previous.saves.clone(), target.clone())],
                u64::MAX,
            )
            .await?;
            assert_eq!(fs::read_dir(&fixture.live)?.count(), 0);
            assert_eq!(
                fs::read(fixture.bank(&previous.saves)?.join("data/save.dat"))?,
                b"current progress"
            );
            let current = fixture.state().await?;
            assert_eq!(current.generation, previous.generation);
            assert_eq!(current.profile, previous.profile);
            assert_eq!(current.modified, previous.modified);
            assert_eq!(current.saves, target);
            super::super::super::save_switch::switch(
                &fixture.history.tracker,
                &[(fixture.game.clone(), target, previous.saves.clone())],
                u64::MAX,
            )
            .await?;
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert_eq!(fixture.state().await?, previous);
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn save_only_recovery_follows_commit_and_preserves_external_edits() -> Result<()> {
    isolated(
        "switching::save_only_recovery_follows_commit_and_preserves_external_edits",
        async |root| {
            for committed in [false, true] {
                for edited in [false, true] {
                    let case = root.join(format!("{committed}-{edited}"));
                    fs::create_dir(&case)?;
                    let fixture = Fixture::new(case).await?;
                    let previous = fixture.state().await?;
                    let target = SaveSetId::Profile {
                        game_id: fixture.game.id.clone(),
                        profile_id: fixture.profile.clone(),
                    };
                    fs::write(fixture.live.join("save.dat"), b"target character")?;
                    save_manager::initialize_save_set(&fixture.game, &target).await?;
                    fs::write(fixture.live.join("save.dat"), b"outgoing character")?;
                    let id = uuid::Uuid::new_v4().to_string();
                    let transition = save_manager::activation::prepare_sync(
                        &id,
                        &fixture.game,
                        &previous.saves,
                        &target,
                        u64::MAX,
                    )
                    .await?;
                    let document =
                        serde_json::json!({"id":id,"previous":previous,"transition":transition});
                    sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES (?,?,'restore',2,?)")
                    .bind(&id).bind(&fixture.game.id).bind(document.to_string()).execute(&fixture.history.tracker.pool).await?;
                    transition.adopt_preparation(&fixture.game).await?;
                    transition.apply(&fixture.game).await?;
                    if committed {
                        let mut tx = durable(&fixture.history.tracker).await?;
                        sqlx::query("UPDATE generation_game_state SET live_save_profile_id=?,live_save_mode='profile' WHERE game_id=?")
                        .bind(&fixture.profile).bind(&fixture.game.id).execute(&mut *tx).await?;
                        sqlx::query("UPDATE generation_journals SET committed=1 WHERE id=?")
                            .bind(&id)
                            .execute(&mut *tx)
                            .await?;
                        tx.commit().await?;
                    }
                    let target_modified =
                        fs::metadata(fixture.live.join("save.dat"))?.modified()?;
                    if edited {
                        fs::write(fixture.live.join("save.dat"), b"external edit")?;
                    }
                    assert_eq!(
                        super::super::super::recovery::recover_journal(
                            &fixture.history,
                            &fixture.game
                        )
                        .await
                        .is_err(),
                        edited
                    );
                    assert_eq!(
                        fs::read(fixture.live.join("save.dat"))?,
                        if edited {
                            b"external edit".as_slice()
                        } else if committed {
                            b"target character".as_slice()
                        } else {
                            b"outgoing character".as_slice()
                        }
                    );
                    let pending: i64 =
                        sqlx::query_scalar("SELECT count(*) FROM generation_journals")
                            .fetch_one(&fixture.history.tracker.pool)
                            .await?;
                    assert_eq!(pending, i64::from(edited));
                    if edited {
                        fs::write(fixture.live.join("save.dat"), b"target character")?;
                        fs::File::open(fixture.live.join("save.dat"))?
                            .set_times(fs::FileTimes::new().set_modified(target_modified))?;
                        super::super::super::recovery::recover_journal(
                            &fixture.history,
                            &fixture.game,
                        )
                        .await?;
                    }
                    super::super::super::recovery::recover_journal(&fixture.history, &fixture.game)
                        .await?;
                }
            }
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn interrupted_save_sync_recovers_without_a_mod_generation_store() -> Result<()> {
    isolated(
        "switching::interrupted_save_sync_recovers_without_a_mod_generation_store",
        async |root| {
            let fixture = Fixture::legacy(root).await?;
            let tracker = fixture.history.tracker.clone();
            let previous = super::super::super::ownership::initialize(&fixture.history).await?;
            sqlx::query("DELETE FROM generation_stores WHERE game_id=?")
                .bind(&fixture.game.id)
                .execute(&tracker.pool)
                .await?;
            let target = SaveSetId::Profile {
                game_id: fixture.game.id.clone(),
                profile_id: fixture.profile.clone(),
            };
            let id = uuid::Uuid::new_v4().to_string();
            let transition = save_manager::activation::prepare_sync(
                &id,
                &fixture.game,
                &previous.saves,
                &target,
                u64::MAX,
            )
            .await?;
            let document = serde_json::json!({"id":id,"previous":previous,"transition":transition});
            sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES (?,?,'restore',2,?)")
            .bind(&id).bind(&fixture.game.id).bind(document.to_string()).execute(&tracker.pool).await?;
            transition.adopt_preparation(&fixture.game).await?;
            transition.apply(&fixture.game).await?;
            assert_eq!(fs::read_dir(&fixture.live)?.count(), 0);
            drop(fixture.history);
            super::super::super::session::initialize(&tracker, &fixture.game).await?;
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert_eq!(
                super::super::super::session::live_saves(&tracker, &fixture.game).await?,
                previous.saves
            );
            let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM generation_journals")
                .fetch_one(&tracker.pool)
                .await?;
            assert_eq!(pending, 0);
            Ok(())
        },
    )
}
