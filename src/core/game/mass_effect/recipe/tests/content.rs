use super::*;

const OFFICIAL: &str = "BioGame/DLC/DLC_UPD_Patch01/CookedPCConsole/Existing.pcc";
const ADDED: &str = "BioGame/DLC/DLC_UPD_Patch01/CookedPCConsole/Added.pcc";
const UNTOUCHED: &str = "BioGame/DLC/DLC_UPD_Patch01/CookedPCConsole/Unchanged.pcc";

async fn fixture(target: Target) -> Result<Fixture> {
    Fixture::with_game(target, |root| {
        fs::create_dir_all(root.join(OFFICIAL).parent().context("missing parent")?)?;
        fs::write(root.join(OFFICIAL), b"official original")?;
        fs::write(root.join(UNTOUCHED), b"untouched original")?;
        Ok(())
    })
    .await
}

async fn retain(fixture: &Fixture, content: &[u8], structured: bool) -> Result<Package> {
    let source = fixture.temp.path().join(Uuid::new_v4().to_string());
    for path in [OFFICIAL, ADDED] {
        let file = source.join("Payload").join(path);
        fs::create_dir_all(file.parent().context("missing parent")?)?;
        fs::write(file, content)?;
    }
    let target = fixture.recipe(Vec::new()).target;
    let target = if target == Target::Le2 { "LE2" } else { "LE3" };
    let mapping = if structured {
        "gamedirectorystructure=true\nnewfiles=.\nreplacefiles=.\n".into()
    } else {
        format!(
            "newfiles={OFFICIAL}\nreplacefiles={OFFICIAL}\naddfiles={ADDED}\naddfilestargets={ADDED}\n"
        )
    };
    fs::write(
        source.join("moddesc.ini"),
        format!(
            "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame={target}\nmodname=Content\nmodver=1.0\nmoddev=Author\nmoddesc=Content\n[BASEGAME]\nmoddir=Payload\n{mapping}"
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

// @variants: both
#[tokio::test]
async fn manual_official_dlc_files_use_the_same_restoration_boundary() -> Result<()> {
    for target in [Target::Le2, Target::Le3] {
        let fixture = fixture(target).await?;
        let source = fixture.temp.path().join("manual");
        let file = source.join(OFFICIAL);
        fs::create_dir_all(file.parent().context("missing parent")?)?;
        fs::write(file, b"manual content")?;
        assert!(PackagePlan::inspect(&source, None).is_err());
        let plan = PackagePlan::inspect(&source, Some(target))?;
        let package = sources::retain_in(
            source,
            plan,
            None,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?
        .selection();
        let installed = fixture.deploy(fixture.recipe(vec![package]), None).await?;
        assert_eq!(
            fs::read(fixture.game.path.join(OFFICIAL))?,
            b"manual content"
        );
        assert_eq!(
            fs::read(fixture.game.path.join(UNTOUCHED))?,
            b"untouched original"
        );
        fixture
            .deploy(fixture.recipe(Vec::new()), Some(&installed))
            .await?;
        assert_eq!(
            fs::read(fixture.game.path.join(OFFICIAL))?,
            b"official original"
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn official_dlc_edits_reorder_disable_and_restore_without_changing_sources() -> Result<()> {
    for target in [Target::Le2, Target::Le3] {
        let fixture = fixture(target).await?;
        let first = retain(&fixture, b"first", false).await?;
        let second = retain(&fixture, b"second", true).await?;
        let recipe = fixture.recipe(vec![first.clone(), second.clone()]);
        let plan = fixture.inspect(recipe.clone()).await?;
        assert_eq!(plan.collisions().len(), 2);
        assert!(
            plan.collisions()
                .iter()
                .all(|collision| collision.winner == second.id)
        );
        let installed = fixture.deploy(recipe, None).await?;
        assert_eq!(fs::read(fixture.game.path.join(OFFICIAL))?, b"second");
        assert_eq!(fs::read(fixture.game.path.join(ADDED))?, b"second");
        assert_eq!(
            fs::read(fixture.game.path.join(UNTOUCHED))?,
            b"untouched original"
        );
        let reordered = fixture
            .deploy(
                fixture.recipe(vec![second.clone(), first.clone()]),
                Some(&installed),
            )
            .await?;
        assert_eq!(fs::read(fixture.game.path.join(OFFICIAL))?, b"first");
        let mut disabled = first.clone();
        disabled.enabled = false;
        let rebuilt = fixture
            .deploy(
                fixture.recipe(vec![second.clone(), disabled]),
                Some(&reordered),
            )
            .await?;
        assert_eq!(fs::read(fixture.game.path.join(OFFICIAL))?, b"second");
        fixture
            .deploy(fixture.recipe(Vec::new()), Some(&rebuilt))
            .await?;
        assert_eq!(
            fs::read(fixture.game.path.join(OFFICIAL))?,
            b"official original"
        );
        assert!(!fixture.game.path.join(ADDED).exists());
        assert_eq!(
            fs::read(fixture.game.path.join(UNTOUCHED))?,
            b"untouched original"
        );
        assert_eq!(
            fs::read(fixture.stored(&first, &format!("Payload/{OFFICIAL}")))?,
            b"first"
        );
        assert_eq!(
            fs::read(fixture.stored(&second, &format!("Payload/{OFFICIAL}")))?,
            b"second"
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
async fn official_dlc_changes_roll_back_on_cancellation_and_database_failure() -> Result<()> {
    let fixture = fixture(Target::Le3).await?;
    let first = retain(&fixture, b"first", false).await?;
    let installed = fixture.deploy(fixture.recipe(vec![first]), None).await?;
    let second = retain(&fixture, b"second", true).await?;
    let recipe = fixture.recipe(vec![second]);
    let plan = fixture.inspect(recipe.clone()).await?;
    let cancelled = cancel();
    let signal = cancelled.clone();
    assert!(
        deploy_in(
            fixture.tracker.clone(),
            Destination {
                repair_components: false,
                backend: None,
                game: fixture.game.clone(),
                profile: fixture.profile.clone(),
                previous: Some(installed.generation.clone()),
            },
            plan,
            fixture.data.clone(),
            cancelled,
            Arc::new(move |done, total| {
                if done > 0 && done < total {
                    signal.store(true, std::sync::atomic::Ordering::Release);
                }
            })
        )
        .await
        .is_err()
    );
    for path in [OFFICIAL, ADDED] {
        assert_eq!(fs::read(fixture.game.path.join(path))?, b"first");
    }
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(installed.clone())
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    sqlx::query("CREATE TRIGGER reject_recipe BEFORE UPDATE ON mele_recipes BEGIN SELECT RAISE(ABORT, 'recipe failure'); END").execute(&fixture.tracker.pool).await?;
    assert!(fixture.deploy(recipe, Some(&installed)).await.is_err());
    for path in [OFFICIAL, ADDED] {
        assert_eq!(fs::read(fixture.game.path.join(path))?, b"first");
    }
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(installed)
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

// @variants: both
#[tokio::test]
async fn official_dlc_preflight_rejects_missing_baselines_and_preserves_external_edits()
-> Result<()> {
    let absent = Fixture::new(Target::Le2).await?;
    let package = retain(&absent, b"mod", false).await?;
    assert!(
        absent
            .inspect(absent.recipe(vec![package]))
            .await
            .err()
            .context("accepted missing DLC")?
            .to_string()
            .contains("absent from the restoration baseline")
    );
    let fixture = fixture(Target::Le2).await?;
    let package = retain(&fixture, b"mod", true).await?;
    let recipe = fixture.recipe(vec![package]);
    let installed = fixture.deploy(recipe.clone(), None).await?;
    fs::write(fixture.game.path.join(OFFICIAL), b"external edit")?;
    assert!(fixture.deploy(recipe, Some(&installed)).await.is_err());
    assert_eq!(
        fs::read(fixture.game.path.join(OFFICIAL))?,
        b"external edit"
    );
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(installed)
    );
    Ok(())
}
