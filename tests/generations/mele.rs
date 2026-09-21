use std::fs;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::core::game::mass_effect::{
    baseline, generations::Snapshot, journal::Journal as Mele, package::SourceFile, recipe::Recipe,
};
use crate::core::save_manager::SaveSetId;
use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameConfig, GameEngine};

use super::catalog::{History, durable};
use super::content::Control;
use super::journal::{Journal, Node};
use super::manifest::{self, Output};
use super::state::{self, Deployment};
use super::target::Target;

const ENGINE: &str = "BioGame/CookedPCConsole/Engine.pcc";

fn create_launcher(root: &std::path::Path) -> Result<()> {
    fs::create_dir_all(root.join("Game/Launcher"))?;
    fs::write(
        root.join("Game/Launcher/MassEffectLauncher.exe"),
        b"launcher",
    )?;
    fs::write(root.join("Game/Launcher/bink2w64.dll"), b"bink")?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn mele_activation_commits_engine_state_with_history_and_recovers_external_edits()
-> Result<()> {
    for (number, outcome) in [
        (1, 0),
        (1, 1),
        (1, 2),
        (2, 0),
        (2, 1),
        (2, 2),
        (3, 0),
        (3, 1),
        (3, 2),
    ] {
        let temp = tempfile::tempdir()?;
        let family_root = temp.path().join("family");
        let game = Game {
            id: format!("mass-effect-le{number}"),
            title: "MELE".into(),
            path: family_root.join(format!("Game/ME{number}")),
            data_subdir: "BioGame".into(),
            engine: GameEngine::MassEffect,
            wine_prefix: None,
        };
        fs::create_dir_all(game.path.join("Binaries/Win64"))?;
        fs::create_dir_all(game.path.join("BioGame/CookedPCConsole"))?;
        fs::write(
            game.path
                .join(format!("Binaries/Win64/MassEffect{number}.exe")),
            b"executable",
        )?;
        fs::write(game.path.join(ENGINE), b"original")?;
        create_launcher(&family_root)?;
        let tracker = Tracker::open(&format!(
            "sqlite://{}?mode=rwc",
            temp.path().join("tracker.db").display()
        ))
        .await?
        .tracker;
        baseline::configure(
            &tracker,
            &[GameConfig {
                game: game.clone(),
                custom: true,
                locations: vec![crate::utils::location::FolderSelection {
                    role: crate::utils::location::FolderRole::Game,
                    location: crate::utils::location::SelectedLocation {
                        root: family_root,
                        host_hint: None,
                    },
                    relative: format!("Game/ME{number}").into(),
                }],
            }],
            &[],
            Arc::new(|_| {}),
        )
        .await?;
        let profile = tracker.ensure_default_profile(&game.id).await?.id;
        let baseline = tracker
            .load_mele_baseline(&game.id)
            .await?
            .context("Missing fixture baseline")?;
        let data = temp.path().join("data");
        fs::create_dir_all(&data)?;
        let history = History::open(&tracker, &game.id, &data, true).await?;
        let mut manifest =
            manifest::capture(&history, &game, &profile, data.clone(), Control::default()).await?;
        let payload = temp.path().join("payload");
        fs::write(&payload, b"generated output")?;
        let identity = history.retain(payload, Control::default()).await?;
        let recipe: Recipe = serde_json::from_value(
            json!({"version":1,"target":format!("LE{number}"),"backend_version":1,"helper_version":null,"language":"INT","packages":[],"components":[]}),
        )?;
        let file = SourceFile {
            relative: ENGINE.into(),
            size: identity.size,
            sha256: identity.sha256.clone(),
        };
        let snapshot = Snapshot {
            recipe,
            files: vec![file],
            removals: Default::default(),
            required: vec![],
        };
        let activation = uuid::Uuid::new_v4().to_string();
        let desired = snapshot.state(activation.clone(), profile.clone());
        let engine: Mele = serde_json::from_value(
            json!({"version":5,"id":activation,"game_id":game.id,"baseline":baseline.sha256,"previous":null,"desired":desired,"operations":[{"path":ENGINE,"before":{"size":8,"sha256":format!("{:x}",Sha256::digest(b"original"))},"after":{"size":identity.size,"sha256":identity.sha256}}],"directories":[]}),
        )?;
        let mut journal = Journal::prepare(
            &history,
            &game,
            vec![(
                Target::MassEffect {
                    path: ENGINE.into(),
                },
                Node::File {
                    identity: identity.clone(),
                    mode: 0o644,
                },
            )],
            Control::default(),
        )
        .await?;
        journal.attach_mele(&game, engine.clone())?;
        let shared = super::shared::prepare(
            &history,
            &game,
            snapshot.recipe.clone(),
            data,
            Control::default(),
        )
        .await?;
        journal.attach_game_shared(&history, &game, shared).await?;
        manifest.version = 2;
        manifest.mele = Some(snapshot);
        manifest.shared_revision = super::shared::identity(journal.dependency.as_ref())?;
        manifest.outputs.push(Output {
            target: Target::MassEffect {
                path: ENGINE.into(),
            },
            content: Some(identity),
            mode: 0o644,
            mod_id: None,
        });
        let saves = SaveSetId::Global {
            game_id: game.id.clone(),
        };
        let mut tx = durable(&tracker).await?;
        sqlx::query("INSERT INTO generation_game_state(game_id,live_save_mode,modified) VALUES (?,'global',0)").bind(&game.id).execute(&mut *tx).await?;
        tx.commit().await?;
        let mut tx = durable(&tracker).await?;
        let previous = state::read(&mut tx, &game.id).await?;
        tx.rollback().await?;
        if outcome != 0 {
            journal
                .persist(&history, &game, "deploy", manifest.objects())
                .await?;
            let applied = journal.apply(&history, &game, Control::default()).await?;
            let committed = outcome == 2;
            if committed {
                applied
                    .commit(
                        &history,
                        &game,
                        previous.as_ref(),
                        Some(&Deployment {
                            manifest: &manifest,
                            profile: &profile,
                            files: &[],
                        }),
                        &saves,
                    )
                    .await?;
            }
            fs::write(game.path.join(ENGINE), b"external")?;
            assert!(journal.recover(&history, &game, committed).await.is_err());
            assert_eq!(fs::read(game.path.join(ENGINE))?, b"external");
            fs::write(game.path.join(ENGINE), b"generated output")?;
            journal.recover(&history, &game, committed).await?;
            assert_eq!(
                fs::read(game.path.join(ENGINE))?,
                if committed {
                    b"generated output".as_slice()
                } else {
                    b"original".as_slice()
                }
            );
            assert_eq!(
                tracker.mele_deployment(&game.id).await?,
                committed.then_some(engine.desired)
            );
        } else {
            super::coordinator::activate(
                &history,
                &game,
                &journal,
                previous.as_ref(),
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &[],
                }),
                &saves,
                Control::default(),
            )
            .await?;
            assert_eq!(
                tracker.mele_deployment(&game.id).await?,
                Some(engine.desired)
            );
            assert_eq!(history.load(&manifest.id()?).await?, manifest);
            assert_eq!(
                tracker.mele_recipe(&game.id, &profile).await?,
                manifest.mele.map(|snapshot| snapshot.recipe)
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM generation_journals")
                .fetch_one(&tracker.pool)
                .await?,
            0
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn mele_history_restores_complete_sources_and_reuses_outputs_without_a_helper() -> Result<()>
{
    use crate::core::game::mass_effect::{library, package::PackagePlan, recipe::Destination};
    use std::os::unix::fs::MetadataExt;

    for number in 1..=3 {
        let temp = tempfile::tempdir()?;
        let data = temp.path().join("data");
        let family_root = temp.path().join("family");
        let game = Game {
            id: format!("mass-effect-le{number}"),
            title: "MELE".into(),
            path: family_root.join(format!("Game/ME{number}")),
            data_subdir: "BioGame".into(),
            engine: GameEngine::MassEffect,
            wine_prefix: None,
        };
        fs::create_dir_all(game.path.join("BioGame/CookedPCConsole"))?;
        fs::create_dir_all(game.path.join("Binaries/Win64"))?;
        fs::write(
            game.path
                .join(format!("Binaries/Win64/MassEffect{number}.exe")),
            b"executable",
        )?;
        fs::write(game.path.join(ENGINE), b"original")?;
        create_launcher(&family_root)?;
        let database = format!(
            "sqlite://{}?mode=rwc",
            temp.path().join("tracker.db").display()
        );
        let tracker = Tracker::open(&database).await?.tracker;
        baseline::configure(
            &tracker,
            &[GameConfig {
                game: game.clone(),
                custom: true,
                locations: vec![crate::utils::location::FolderSelection {
                    role: crate::utils::location::FolderRole::Game,
                    location: crate::utils::location::SelectedLocation {
                        root: family_root,
                        host_hint: None,
                    },
                    relative: format!("Game/ME{number}").into(),
                }],
            }],
            &[],
            Arc::new(|_| {}),
        )
        .await?;
        let profile = tracker.ensure_default_profile(&game.id).await?.id;
        for (name, enabled, priority) in [("loser", 1, 1), ("winner", 1, 2), ("disabled", 0, 3)] {
            let id = uuid::Uuid::new_v4().to_string();
            let source = temp.path().join(format!("source-{id}"));
            fs::create_dir_all(source.join("DLC_MOD_Test/CookedPCConsole"))?;
            fs::write(source.join("DLC_MOD_Test/CookedPCConsole/Test.pcc"), name)?;
            fs::write(
                source.join("moddesc.ini"),
                &format!(
                    "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE{number}\nmodname=Example\nmodver=1.0\nmoddev=Author\nmoddesc=Content\n[CUSTOMDLC]\nsourcedirs=DLC_MOD_Test\ndestdirs=DLC_MOD_Test\n[BASEGAME]\nmoddir=.\nnewfiles=DLC_MOD_Test/CookedPCConsole/Test.pcc\nreplacefiles=BioGame/CookedPCConsole/Engine.pcc\n"
                ),
            )?;
            let plan = PackagePlan::inspect(&source, None)?;
            let retained = data.join("mele-sources").join(&plan.source_sha256);
            fs::create_dir_all(retained.parent().context("Missing source parent")?)?;
            fs::rename(source, &retained)?;
            let record: library::Record = serde_json::from_value(
                json!({"version":1,"target":format!("LE{number}"),"package":{"id":id,"source_sha256":plan.source_sha256,"archive_sha256":null,"manifest_version":plan.manifest.format,"mod_version":"1.0","enabled":true,"options":[]}}),
            )?;
            sqlx::query("INSERT INTO mods(id,game_id,name,enabled,priority) VALUES (?,?,?,?,?)")
                .bind(&id)
                .bind(&game.id)
                .bind(&id)
                .bind(enabled)
                .bind(priority)
                .execute(&tracker.pool)
                .await?;
            sqlx::query(
                "INSERT INTO profile_mods(profile_id,mod_id,enabled,priority) VALUES (?,?,?,?)",
            )
            .bind(&profile)
            .bind(&id)
            .bind(enabled)
            .bind(priority)
            .execute(&tracker.pool)
            .await?;
            sqlx::query("INSERT INTO mele_packages(mod_id,document) VALUES (?,?)")
                .bind(&id)
                .bind(serde_json::to_string(&record)?)
                .execute(&tracker.pool)
                .await?;
            sqlx::query("INSERT INTO mod_files(mod_id,game_rel_lowercase,game_rel_original,cache_path) VALUES (?,?,?,?)").bind(&id).bind(ENGINE.to_lowercase()).bind(ENGINE).bind(retained.join("DLC_MOD_Test/CookedPCConsole/Test.pcc").to_str()).execute(&tracker.pool).await?;
        }
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let missing = super::mele::purge(&history, &game, data.clone(), Control::default()).await;
        assert!(
            format!("{:#}", missing.err().context("Untracked purge must fail")?)
                .contains("No recorded MELE deployment")
        );
        assert_eq!(fs::read(game.path.join(ENGINE))?, b"original");

        let mut manifest =
            manifest::capture(&history, &game, &profile, data.clone(), Control::default()).await?;
        let recipe: Recipe = serde_json::from_value(
            json!({"version":1,"target":format!("LE{number}"),"backend_version":1,"language":"INT","packages":[]}),
        )?;
        let journal = super::mele::prepare(
            &history,
            Destination {
                game: game.clone(),
                profile: profile.clone(),
                previous: None,
                repair_components: false,
                backend: None,
            },
            &mut manifest,
            recipe,
            data.clone(),
            Control::default(),
        )
        .await?;
        assert_eq!(fs::read(game.path.join(ENGINE))?, b"original");
        sqlx::query(
        "INSERT INTO generation_game_state(game_id,live_save_mode,modified) VALUES (?,'global',0)",
    )
    .bind(&game.id)
    .execute(&tracker.pool)
    .await?;
        let mut tx = durable(&tracker).await?;
        let previous = state::read(&mut tx, &game.id).await?;
        tx.rollback().await?;
        super::coordinator::activate(
            &history,
            &game,
            &journal,
            previous.as_ref(),
            Some(&Deployment {
                manifest: &manifest,
                profile: &profile,
                files: &[],
            }),
            &SaveSetId::Global {
                game_id: game.id.clone(),
            },
            Control::default(),
        )
        .await?;
        assert_eq!(fs::read(game.path.join(ENGINE))?, b"winner");
        let mut tx = tracker.pool.begin().await?;
        let current = super::records::capture_state(&mut tx, &game.id, &profile, true).await?;
        tx.rollback().await?;
        assert_eq!(
            super::status::projection(&current)?,
            super::status::projection(&manifest.records)?,
            "deployed MELE profiles must not retain pending changes"
        );
        let id = manifest.id()?;
        drop(history);
        tracker.pool.close().await;
        let tracker = Tracker::open(&database).await?.tracker;
        super::session::initialize(&tracker, &game).await?;
        assert_eq!(
            super::session::inspect_live(&tracker, &game).await?,
            Some(0)
        );
        let mut tx = tracker.pool.begin().await?;
        let reopened = super::records::capture_state(&mut tx, &game.id, &profile, true).await?;
        tx.rollback().await?;
        assert_eq!(
            super::status::projection(&reopened)?,
            super::status::projection(&manifest.records)?
        );
        sqlx::query("UPDATE mods SET enabled=1 WHERE game_id=? AND priority=3")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        tracker.pool.close().await;
        let tracker = Tracker::open(&database).await?.tracker;
        let mut tx = tracker.pool.begin().await?;
        let pending = super::records::capture_state(&mut tx, &game.id, &profile, true).await?;
        tx.rollback().await?;
        assert_ne!(
            super::status::projection(&pending)?,
            super::status::projection(&manifest.records)?,
            "undeployed MELE changes must survive reopening the game"
        );
        let history = History::open(&tracker, &game.id, temp.path(), false).await?;
        let ids: Vec<_> = tracker
            .list_mods(&game.id)
            .await?
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        tracker.remove_mele_packages(&game.id, &ids).await?;
        assert!(tracker.list_mods(&game.id).await?.is_empty());
        assert!(tracker.mele_deployment(&game.id).await?.is_some());
        history.load(&id).await?;
        let cancelled = Control::default();
        cancelled
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(
            super::mele::purge(&history, &game, data.clone(), cancelled)
                .await
                .is_err()
        );
        assert_eq!(fs::read(game.path.join(ENGINE))?, b"winner");
        let purge = super::mele::purge(&history, &game, data.clone(), Control::default()).await?;
        let report = super::activation::PurgeReport::from_journal(&purge);
        assert_eq!(report.outcome.vanilla_files_restored, 1);
        assert!(report.outcome.files_removed > 0);
        assert!(!report.already_purged);

        let mut tx = durable(&tracker).await?;
        let current = state::read(&mut tx, &game.id).await?;
        tx.rollback().await?;
        super::coordinator::activate(
            &history,
            &game,
            &purge,
            current.as_ref(),
            None,
            &SaveSetId::Global {
                game_id: game.id.clone(),
            },
            Control::default(),
        )
        .await?;
        assert_eq!(fs::read(game.path.join(ENGINE))?, b"original");
        assert!(
            !game
                .path
                .join("BioGame/DLC/DLC_MOD_Test/CookedPCConsole/Test.pcc")
                .exists()
        );
        drop(history);
        tracker.pool.close().await;
        let tracker = Tracker::open(&database).await?.tracker;
        let history = History::open(&tracker, &game.id, temp.path(), false).await?;
        assert!(
            tracker
                .mele_deployment(&game.id)
                .await?
                .context("Missing purged state after restart")?
                .files
                .is_empty()
        );
        history.load(&id).await?;
        let repeated =
            super::mele::purge(&history, &game, data.clone(), Control::default()).await?;
        let report = super::activation::PurgeReport::from_journal(&repeated);
        assert!(report.already_purged);
        assert!(report.message().contains("already purged"));
        sqlx::query("DELETE FROM profiles WHERE id=?")
            .bind(&profile)
            .execute(&tracker.pool)
            .await?;
        fs::remove_dir_all(data.join("mele-sources"))?;
        let restored =
            super::restore::restore(&history, &id, "Restored", Control::default()).await?;
        let mut draft =
            manifest::capture(&history, &game, &restored, data.clone(), Control::default()).await?;
        assert_eq!(
            draft
                .sources
                .iter()
                .filter(|source| source.content.is_some())
                .count(),
            6
        );
        for source in draft
            .sources
            .iter()
            .filter(|source| source.content.is_some())
        {
            let path = temp.path().join(
                source
                    .path
                    .strip_prefix("cache/")
                    .context("Missing writable anchor")?,
            );
            assert_eq!(fs::metadata(&path)?.nlink(), 1);
        }
        let restored_journal = super::mele::reuse(
            &history,
            &game,
            &mut draft,
            data.clone(),
            Control::default(),
        )
        .await?
        .context("Unchanged restoration should reuse retained outputs")?;
        assert_eq!(
            restored_journal
                .mele
                .as_ref()
                .context("Missing engine participant")?
                .desired
                .files,
            manifest
                .mele
                .as_ref()
                .context("Missing retained snapshot")?
                .files
        );
        assert_eq!(fs::read(game.path.join(ENGINE))?, b"original");
        tracker
            .set_profile_save_mode(&restored, crate::models::profile::SaveMode::Global)
            .await?;
        tracker.switch_profile(&game.id, &restored).await?;
        let recipe = draft
            .mele
            .as_ref()
            .context("Missing restored recipe")?
            .recipe
            .clone();
        draft =
            manifest::capture(&history, &game, &restored, data.clone(), Control::default()).await?;
        assert!(
            super::mele::reuse(
                &history,
                &game,
                &mut draft,
                data.clone(),
                Control::default()
            )
            .await?
            .is_none()
        );
        let previous = tracker
            .mele_deployment(&game.id)
            .await?
            .context("Missing purged deployment")?;
        let restored_journal = super::mele::prepare(
            &history,
            Destination {
                game: game.clone(),
                profile: restored.clone(),
                previous: Some(previous.generation),
                repair_components: false,
                backend: None,
            },
            &mut draft,
            recipe,
            data.clone(),
            Control::default(),
        )
        .await?;
        let mut tx = durable(&tracker).await?;
        let current = state::read(&mut tx, &game.id).await?;
        tx.rollback().await?;
        super::coordinator::activate(
            &history,
            &game,
            &restored_journal,
            current.as_ref(),
            Some(&Deployment {
                manifest: &draft,
                profile: &restored,
                files: &[],
            }),
            &SaveSetId::Global {
                game_id: game.id.clone(),
            },
            Control::default(),
        )
        .await?;
        assert_eq!(fs::read(game.path.join(ENGINE))?, b"winner");
        let edited = draft
            .sources
            .iter()
            .find(|source| source.path.ends_with("Test.pcc"))
            .context("Missing writable source")?;
        fs::write(
            temp.path().join(
                edited
                    .path
                    .strip_prefix("cache/")
                    .context("Missing cache anchor")?,
            ),
            b"edited",
        )?;
        let mut changed =
            manifest::capture(&history, &game, &restored, data.clone(), Control::default()).await?;
        assert!(
            super::mele::reuse(&history, &game, &mut changed, data, Control::default())
                .await?
                .is_none()
        );
        history.load(&id).await?;
        let purge = super::mele::purge(
            &history,
            &game,
            temp.path().join("data"),
            Control::default(),
        )
        .await?;
        let report = super::activation::PurgeReport::from_journal(&purge);
        assert!(!report.already_purged);
        assert_eq!(report.outcome.vanilla_files_restored, 1);
        let mut tx = durable(&tracker).await?;
        let current = state::read(&mut tx, &game.id).await?;
        tx.rollback().await?;
        super::coordinator::activate(
            &history,
            &game,
            &purge,
            current.as_ref(),
            None,
            &SaveSetId::Global {
                game_id: game.id.clone(),
            },
            Control::default(),
        )
        .await?;
        assert_eq!(fs::read(game.path.join(ENGINE))?, b"original");
        assert!(
            tracker
                .mele_deployment(&game.id)
                .await?
                .context("Missing purged state")?
                .files
                .is_empty()
        );
        history.load(&id).await?;
        let entry = tracker
            .list_mods(&game.id)
            .await?
            .into_iter()
            .next()
            .context("Missing restored mod")?;
        let record = tracker
            .mele_package(&entry.id)
            .await?
            .context("Missing source binding")?;
        assert!(record.writable_cache);
        sqlx::query("DELETE FROM mod_files WHERE mod_id=?")
            .bind(&entry.id)
            .execute(&tracker.pool)
            .await?;
        let writable = temp.path().join(&entry.id);
        let old = temp
            .path()
            .join("data/mele-sources")
            .join(&record.package.source_sha256);
        fs::create_dir_all(old.parent().context("Missing original cache parent")?)?;
        fs::rename(&writable, &old)?;
        assert!(
            manifest::capture(
                &history,
                &game,
                &restored,
                temp.path().join("data"),
                Control::default()
            )
            .await
            .is_err()
        );
        fs::rename(old, &writable)?;
        manifest::capture(
            &history,
            &game,
            &restored,
            temp.path().join("data"),
            Control::default(),
        )
        .await?;
    }
    Ok(())
}
