use super::*;

const OLD: &str = "BioGame/DLC/DLC_OLD/CookedPCConsole/Old.pcc";
const SECOND: &str = "BioGame/DLC/DLC_OLD/CookedPCConsole/Second.pcc";

async fn fixture(target: Target) -> Result<Fixture> {
    Fixture::with_game(target, |root| {
        fs::create_dir_all(root.join("BioGame/DLC/DLC_OLD/CookedPCConsole/Empty"))?;
        fs::write(root.join(OLD), b"old original")?;
        fs::write(root.join(SECOND), b"second original")?;
        Ok(())
    })
    .await
}

async fn retiring(fixture: &Fixture, names: &str) -> Result<Package> {
    let source = fixture.temp.path().join(Uuid::new_v4().to_string());
    fs::create_dir(&source)?;
    let target = match fixture.recipe(Vec::new()).target {
        Target::Le1 => "LE1",
        Target::Le2 => "LE2",
        Target::Le3 => "LE3",
    };
    fs::write(
        source.join("moddesc.ini"),
        format!(
            "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame={target}\nmodname=Replacement\nmodver=1.0\nmoddev=Author\nmoddesc=Retires old content\n[CUSTOMDLC]\noutdatedcustomdlc={names}\n"
        ),
    )?;
    let plan = PackagePlan::inspect(&source, None)?;
    Ok(sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection())
}

async fn replacement(fixture: &Fixture) -> Result<Package> {
    let source = fixture.temp.path().join(Uuid::new_v4().to_string());
    let file = source.join(OLD);
    fs::create_dir_all(file.parent().context("missing parent")?)?;
    fs::write(file, b"new content")?;
    let plan = PackagePlan::inspect(&source, Some(fixture.recipe(Vec::new()).target))?;
    Ok(sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection())
}

// @variants: both
#[tokio::test]
async fn obsolete_dlc_is_removed_rebuilt_and_restored_after_reopening_the_database() -> Result<()> {
    for target in [Target::Le1, Target::Le2, Target::Le3] {
        let mut fixture = fixture(target).await?;
        let retiring = retiring(&fixture, "dlc_old").await?;
        let recipe = fixture.recipe(vec![retiring.clone()]);
        let installed = fixture.deploy(recipe.clone(), None).await?;
        assert_eq!(installed.version, 3);
        assert_eq!(
            installed.removals.paths,
            BTreeSet::from([OLD.into(), SECOND.into()])
        );
        assert!(!fixture.game.path.join("BioGame/DLC/DLC_OLD").exists());
        fixture.tracker = Tracker::open(&format!(
            "sqlite://{}?mode=rwc",
            fixture.temp.path().join("tracker.db").display()
        ))
        .await?
        .tracker;
        assert_eq!(
            fixture.tracker.mele_deployment(&fixture.game.id).await?,
            Some(installed.clone())
        );
        let rebuilt = fixture.deploy(recipe, Some(&installed)).await?;
        assert!(!fixture.game.path.join(OLD).exists());
        let mut disabled = retiring;
        disabled.enabled = false;
        let restored = fixture
            .deploy(fixture.recipe(vec![disabled]), Some(&rebuilt))
            .await?;
        assert!(restored.removals.is_empty());
        assert_eq!(fs::read(fixture.game.path.join(OLD))?, b"old original");
        assert_eq!(
            fs::read(fixture.game.path.join(SECOND))?,
            b"second original"
        );
        assert_eq!(
            fixture
                .tracker
                .load_mele_baseline(&fixture.game.id)
                .await?
                .context("missing baseline")?
                .sha256,
            fixture.baseline.sha256
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn obsolete_dlc_follows_package_order_and_excludes_retired_merge_contributions() -> Result<()>
{
    let fixture = fixture(Target::Le1).await?;
    let retiring = retiring(&fixture, "DLC_OLD;DLC_MOD_Merge").await?;
    let replacement = replacement(&fixture).await?;
    let installed = fixture
        .deploy(
            fixture.recipe(vec![replacement.clone(), retiring.clone()]),
            None,
        )
        .await?;
    assert!(!fixture.game.path.join(OLD).exists());
    let reordered = fixture
        .deploy(
            fixture.recipe(vec![retiring.clone(), replacement]),
            Some(&installed),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(OLD))?, b"new content");
    assert!(!fixture.game.path.join(SECOND).exists());
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&reordered))
        .await?;
    assert_eq!(
        fs::read(fixture.game.path.join(SECOND))?,
        b"second original"
    );
    let merge = fixture
        .merge_package("DLC_MOD_Merge", 10, &["table", "plot", "config"])
        .await?;
    let plan = fixture
        .inspect(fixture.merge_recipe(vec![merge, retiring]))
        .await?;
    assert!(plan.files().is_empty());
    for phase in 0..3 {
        assert!(plan.merges.job(phase, &BTreeMap::new())?.is_none());
    }
    let state = fixture
        .deploy(
            fixture.recipe(Vec::new()),
            fixture
                .tracker
                .mele_deployment(&fixture.game.id)
                .await?
                .as_ref(),
        )
        .await?;
    assert!(state.removals.is_empty());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn obsolete_dlc_is_removed_from_staging_after_installation_transformations() -> Result<()> {
    let fixture = fixture(Target::Le1).await?;
    let old = fixture
        .merge_package("DLC_MOD_Merge", 10, &["table"])
        .await?;
    let update = fixture.installer_package(true, true, &[], "9.1").await?;
    let retiring = retiring(&fixture, "DLC_OLD;DLC_MOD_Merge").await?;
    let recipe = fixture.merge_recipe(vec![old, update, retiring]);
    let installed = fixture
        .deploy_merges(
            recipe,
            None,
            fixture.backend("")?,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert!(!fixture.game.path.join("BioGame/DLC/DLC_OLD").exists());
    assert!(
        installed
            .files
            .iter()
            .all(|file| !file.relative.starts_with("BioGame/DLC/DLC_OLD/"))
    );
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&installed))
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(OLD))?, b"old original");
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn obsolete_dlc_preserves_unmanaged_files_links_and_reappearing_content() -> Result<()> {
    let fixture = fixture(Target::Le2).await?;
    let package = retiring(&fixture, "DLC_OLD").await?;
    let recipe = fixture.recipe(vec![package]);
    let unexpected = fixture.game.path.join("BioGame/DLC/DLC_OLD/user.txt");
    fs::write(&unexpected, b"user data")?;
    assert!(fixture.deploy(recipe.clone(), None).await.is_err());
    assert_eq!(fs::read(&unexpected)?, b"user data");
    assert_eq!(fs::read(fixture.game.path.join(OLD))?, b"old original");
    fs::remove_file(&unexpected)?;
    symlink(fixture.game.path.join(OLD), &unexpected)?;
    assert!(fixture.deploy(recipe.clone(), None).await.is_err());
    assert!(fs::symlink_metadata(&unexpected)?.is_symlink());
    fs::remove_file(&unexpected)?;
    let installed = fixture.deploy(recipe.clone(), None).await?;
    fs::create_dir_all(unexpected.parent().context("missing parent")?)?;
    fs::write(&unexpected, b"late data")?;
    assert!(fixture.deploy(recipe, Some(&installed)).await.is_err());
    assert!(
        fixture
            .deploy(fixture.recipe(Vec::new()), Some(&installed))
            .await
            .is_err()
    );
    assert_eq!(fs::read(unexpected)?, b"late data");
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(installed)
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn obsolete_dlc_removal_rolls_back_cancellation_and_failed_commit() -> Result<()> {
    let fixture = fixture(Target::Le3).await?;
    let package = retiring(&fixture, "DLC_OLD").await?;
    let recipe = fixture.recipe(vec![package]);
    let plan = fixture.inspect(recipe.clone()).await?;
    let cancelled = cancel();
    let signal = cancelled.clone();
    assert!(
        deploy_in(
            fixture.tracker.clone(),
            Destination {
                game: fixture.game.clone(),
                profile: fixture.profile.clone(),
                previous: None,
                repair_components: false,
                backend: None,
            },
            plan,
            fixture.data.clone(),
            cancelled,
            Arc::new(move |done, total| {
                if done > 0 && done < total {
                    signal.store(true, Ordering::Release);
                }
            })
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(fixture.game.path.join(OLD))?, b"old original");
    assert_eq!(
        fs::read(fixture.game.path.join(SECOND))?,
        b"second original"
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    sqlx::query("CREATE TRIGGER reject_recipe BEFORE INSERT ON mele_recipes BEGIN SELECT RAISE(ABORT, 'recipe failure'); END").execute(&fixture.tracker.pool).await?;
    assert!(fixture.deploy(recipe, None).await.is_err());
    assert_eq!(fs::read(fixture.game.path.join(OLD))?, b"old original");
    assert_eq!(
        fs::read(fixture.game.path.join(SECOND))?,
        b"second original"
    );
    assert!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .is_none()
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    Ok(())
}
