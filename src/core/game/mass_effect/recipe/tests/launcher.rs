use super::*;
use crate::core::game::mass_effect::{components, launcher};

fn archive(content: &[u8]) -> Result<launcher::Inspected> {
    let source = tempfile::tempdir()?;
    fs::create_dir_all(source.path().join("Content"))?;
    fs::write(source.path().join("Content/Intro.bik"), content)?;
    let entry = launcher::parse(source.path(), "Intro video")?;
    Ok(launcher::Inspected {
        source: Some(source),
        entry,
    })
}

async fn change(fixture: &Fixture, game: &Game, action: launcher::Action) -> Result<()> {
    launcher::apply_in(
        fixture.tracker.clone(),
        launcher::load(&fixture.tracker, game).await?,
        action,
        fixture.data.clone(),
        cancel(),
    )
    .await
}

// @variants: both
#[tokio::test]
#[ignore = "Uses the pinned launcher proxy from the persistent test cache; disposable games only"]
async fn shared_launcher_mods_preserve_game_profiles_and_restore_for_all_games() -> Result<()> {
    let fixture = Fixture::with_game(Target::Le1, |root| {
        fs::write(root.join(components::BINK), b"game bink")?;
        Ok(())
    })
    .await?;
    let root = fixture.temp.path().join("family/Game/Launcher");
    fs::create_dir_all(root.join("Content"))?;
    fs::write(root.join("Content/Intro.bik"), b"original video")?;
    let required = components::required(Target::Le2)
        .into_iter()
        .filter(|selection| selection.component == components::Component::BinkProxy)
        .collect::<Vec<_>>();
    components::test_cache::seed(&fixture.data, &required).await?;
    let mut games = vec![fixture.game.clone()];
    let location = fixture
        .tracker
        .folder_location(&fixture.game.id, crate::utils::location::FolderRole::Game)
        .await?;
    for (target, number) in [(Target::Le2, 2), (Target::Le3, 3)] {
        let mut game = fixture.game.clone();
        game.id = target.game_id().into();
        game.path = fixture.temp.path().join(format!("family/Game/ME{number}"));
        fs::create_dir_all(game.path.join("Binaries/Win64"))?;
        fs::create_dir_all(game.path.join("BioGame"))?;
        fs::write(
            game.path
                .join(format!("Binaries/Win64/MassEffect{number}.exe")),
            b"exe",
        )?;
        crate::core::game::mass_effect::baseline::configure(
            &fixture.tracker,
            &[GameConfig {
                game: game.clone(),
                custom: true,
                locations: vec![crate::utils::location::FolderSelection {
                    role: crate::utils::location::FolderRole::Game,
                    location: location.selection.clone(),
                    relative: format!("Game/ME{number}").into(),
                }],
            }],
            &[],
            Arc::new(|_| {}),
        )
        .await?;
        fixture.tracker.ensure_default_profile(&game.id).await?;
        games.push(game);
    }
    let (_, package) = fixture.retain(b"game mod", "").await?;
    let deployed = fixture.deploy(fixture.recipe(vec![package]), None).await?;
    let recipe = fixture
        .tracker
        .mele_recipe(&fixture.game.id, &fixture.profile)
        .await?;
    let first = archive(b"first video")?;
    let first_id = first.entry.id.clone();
    change(&fixture, &games[0], launcher::Action::Add(first)).await?;
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"first video");
    for game in &games {
        assert_eq!(
            launcher::load(&fixture.tracker, game).await?.entries[0].id,
            first_id
        );
    }
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(deployed.clone())
    );
    assert_eq!(
        fixture
            .tracker
            .mele_recipe(&fixture.game.id, &fixture.profile)
            .await?,
        recipe
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"game mod");
    let stale = launcher::load(&fixture.tracker, &games[0]).await?;
    let second = archive(b"second video")?;
    let second_id = second.entry.id.clone();
    change(&fixture, &games[1], launcher::Action::Add(second)).await?;
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"second video");
    assert!(
        launcher::apply_in(
            fixture.tracker.clone(),
            stale,
            launcher::Action::Restore,
            fixture.data.clone(),
            cancel()
        )
        .await
        .is_err()
    );
    change(
        &fixture,
        &games[2],
        launcher::Action::Move(second_id.clone(), -1),
    )
    .await?;
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"first video");
    change(
        &fixture,
        &games[0],
        launcher::Action::Enable(first_id.clone(), false),
    )
    .await?;
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"second video");
    let purged = fixture
        .deploy(fixture.recipe(Vec::new()), Some(&deployed))
        .await?;
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"second video");
    assert_eq!(
        launcher::load(&fixture.tracker, &games[2])
            .await?
            .entries
            .len(),
        2
    );
    fs::remove_file(root.join("Content/Intro.bik"))?;
    change(&fixture, &games[1], launcher::Action::Repair).await?;
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"second video");
    fs::write(root.join("Content/Intro.bik"), b"external change")?;
    assert!(
        change(&fixture, &games[1], launcher::Action::Restore)
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(root.join("Content/Intro.bik"))?,
        b"external change"
    );
    fs::write(root.join("Content/Intro.bik"), b"second video")?;
    sqlx::query("CREATE TRIGGER reject_launcher_mods BEFORE UPDATE ON mele_families BEGIN SELECT RAISE(ABORT, 'failure'); END").execute(&fixture.tracker.pool).await?;
    assert!(
        change(&fixture, &games[2], launcher::Action::Restore)
            .await
            .is_err()
    );
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"second video");
    sqlx::query("DROP TRIGGER reject_launcher_mods")
        .execute(&fixture.tracker.pool)
        .await?;
    change(&fixture, &games[2], launcher::Action::Restore).await?;
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"original video");
    assert_eq!(
        fs::read(root.join("bink2w64.dll"))?,
        b"original launcher library"
    );
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(purged)
    );
    change(&fixture, &games[0], launcher::Action::Remove(first_id)).await?;
    change(&fixture, &games[1], launcher::Action::Remove(second_id)).await?;
    assert!(
        launcher::load(&fixture.tracker, &games[2])
            .await?
            .entries
            .is_empty()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Uses the cached pinned proxy; interrupts only a disposable launcher transaction"]
async fn launcher_journals_recover_without_sources_and_preserve_external_edits() -> Result<()> {
    use crate::core::game::mass_effect::{
        family,
        operation::{Control, Lease},
    };
    let fixture = Fixture::new(Target::Le1).await?;
    let root = fixture.temp.path().join("family/Game/Launcher");
    fs::create_dir_all(root.join("Content"))?;
    fs::write(root.join("Content/Intro.bik"), b"original video")?;
    let required = components::required(Target::Le2)
        .into_iter()
        .filter(|selection| selection.component == components::Component::BinkProxy)
        .collect::<Vec<_>>();
    components::test_cache::seed(&fixture.data, &required).await?;
    change(
        &fixture,
        &fixture.game,
        launcher::Action::Add(archive(b"installed video")?),
    )
    .await?;
    let snapshot = launcher::load(&fixture.tracker, &fixture.game).await?;
    let cancelled = cancel();
    cancelled.store(true, Ordering::Release);
    assert!(
        launcher::apply_in(
            fixture.tracker.clone(),
            snapshot.clone(),
            launcher::Action::Restore,
            fixture.data.clone(),
            cancelled
        )
        .await
        .is_err()
    );
    assert_eq!(
        fs::read(root.join("Content/Intro.bik"))?,
        b"installed video"
    );
    let mut entries = snapshot.entries;
    entries[0].enabled = false;
    let control = Control::recovery();
    let lease = Lease::acquire(&control).await?;
    let plan = family::edit(
        &fixture.tracker,
        &fixture.game,
        entries,
        &fixture.data,
        control.clone(),
    )
    .await?;
    let journal = journal::shared::stage(
        &fixture.tracker,
        &fixture.game,
        &plan,
        &fixture.data,
        &control,
    )
    .await?;
    fixture.tracker.begin_mele_journal(&journal).await?;
    let staging = plan.change.storage(&fixture.data, &journal.id);
    plan.change.apply(&root, &staging, &control)?;
    drop(lease);
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"original video");
    let reopened = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture.temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    assert!(
        reopened
            .ensure_no_mele_journal(Target::Le2.game_id())
            .await
            .is_err()
    );
    fs::remove_dir_all(fixture.data.join("mele-launcher-sources"))?;
    fs::write(root.join("Content/Intro.bik"), b"outside edit")?;
    assert!(
        journal::recover_in(reopened.clone(), fixture.game.clone(), fixture.data.clone())
            .await
            .is_err()
    );
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"outside edit");
    assert!(reopened.mele_journal(&fixture.game.id).await?.is_some());
    fs::write(root.join("Content/Intro.bik"), b"original video")?;
    journal::recover_in(reopened.clone(), fixture.game.clone(), fixture.data.clone()).await?;
    assert_eq!(
        fs::read(root.join("Content/Intro.bik"))?,
        b"installed video"
    );
    assert!(reopened.mele_deployment(&fixture.game.id).await?.is_none());
    assert!(reopened.mele_journal(&fixture.game.id).await?.is_none());
    assert!(launcher::load(&reopened, &fixture.game).await?.entries[0].enabled);
    change(&fixture, &fixture.game, launcher::Action::Restore).await?;
    assert_eq!(fs::read(root.join("Content/Intro.bik"))?, b"original video");
    Ok(())
}
