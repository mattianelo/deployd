use super::*;

// @variants: both
#[tokio::test]
async fn preparation_preserves_externally_managed_merge_dlc() -> Result<()> {
    let fixture = Fixture::new(Target::Le2).await?;
    let root = fixture.game.path.join("BioGame/DLC/dlc_mod_m3_merge");
    fs::create_dir_all(&root)?;
    fs::write(root.join("external.pcc"), b"external content")?;
    let result = fixture
        .prepare(fixture.recipe(Vec::new()), None, None)
        .await;
    assert!(result.is_err());
    assert_eq!(fs::read(root.join("external.pcc"))?, b"external content");
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert!(!fixture.data.join("mele-rebuilds").exists());
    assert!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .is_none()
    );
    fs::remove_file(root.join("external.pcc"))?;
    symlink(fixture.game.path.join(ENGINE), root.join("linked.pcc"))?;
    assert!(
        fixture
            .prepare(fixture.recipe(Vec::new()), None, None)
            .await
            .is_err()
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn preparation_rejects_a_baseline_with_an_external_merge_dlc() -> Result<()> {
    let relative = "BioGame/DLC/DLC_MOD_M3_MERGE/CookedPCConsole/External.pcc";
    let fixture = Fixture::with_game(Target::Le3, |game| {
        let file = game.join(relative);
        fs::create_dir_all(file.parent().context("missing fixture parent")?)?;
        fs::write(file, b"external content")?;
        Ok(())
    })
    .await?;
    let result = fixture
        .prepare(fixture.recipe(Vec::new()), None, None)
        .await;
    assert!(result.is_err());
    assert!(
        format!("{:#}", result.err().context("missing rejection")?).contains("restoration point")
    );
    assert_eq!(
        fs::read(fixture.game.path.join(relative))?,
        b"external content"
    );
    assert!(!fixture.data.join("mele-rebuilds").exists());
    Ok(())
}

impl Fixture {
    fn destination(
        &self,
        previous: Option<&journal::State>,
        backend: Option<helper::Backend>,
    ) -> Destination {
        Destination {
            game: self.game.clone(),
            profile: self.profile.clone(),
            previous: previous.map(|state| state.generation.clone()),
            repair_components: false,
            backend,
        }
    }

    async fn prepare(
        &self,
        recipe: Recipe,
        previous: Option<&journal::State>,
        backend: Option<helper::Backend>,
    ) -> Result<ValidatedRecipe> {
        inspect_prepared_in(
            self.tracker.clone(),
            self.destination(previous, backend),
            recipe,
            self.data.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await
    }

    async fn apply_prepared(
        &self,
        plan: ValidatedRecipe,
        previous: Option<&journal::State>,
        backend: Option<helper::Backend>,
    ) -> Result<journal::State> {
        deploy_in(
            self.tracker.clone(),
            self.destination(previous, backend),
            plan,
            self.data.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await
    }

    async fn sized_package(&self, size: u64, required_path: &str) -> Result<Package> {
        let (source, _) = self.package(b"second", "")?;
        fs::create_dir_all(source.join("Extra/CookedPCConsole"))?;
        fs::write(source.join("Extra/CookedPCConsole/Marker.pcc"), b"selected")?;
        let path = source.join("moddesc.ini");
        let text = fs::read_to_string(&path)?.replace("[BASEGAME]", &format!("altdlc=((FriendlyName=Sized,Condition=COND_SPECIFIC_SIZED_FILES,RequiredFileRelativePaths={required_path},RequiredFileSizes={size},ModOperation=OP_ADD_CUSTOMDLC,ModAltDLC=Extra,ModDestDLC=DLC_MOD_Sized))\n[BASEGAME]"));
        fs::write(path, text)?;
        let plan = PackagePlan::inspect(&source, None)?;
        Ok(sources::retain_in(
            source,
            plan,
            None,
            self.data.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?
        .selection())
    }
}

// @variants: both
#[tokio::test]
async fn preparation_resolves_transformed_sizes_and_reuses_verified_outputs() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let first = fixture.installer_package(true, true, &[], "9.2").await?;
    let second = fixture.sized_package(9, ENGINE).await?;
    let backend = fixture.backend("")?;
    let recipe = fixture.merge_recipe(vec![first, second]);
    assert!(fixture.inspect(recipe.clone()).await.is_err());
    let plan = fixture
        .prepare(recipe.clone(), None, Some(backend.clone()))
        .await?;
    let marker = "BioGame/DLC/DLC_MOD_Sized/CookedPCConsole/Marker.pcc";
    assert!(
        plan.files()
            .iter()
            .any(|file| file.destination.relative == marker)
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    fs::write(
        &backend.runtime,
        "#!/usr/bin/python3\nraise SystemExit(97)\n",
    )?;
    let installed = fixture.apply_prepared(plan, None, Some(backend)).await?;
    assert_eq!(fs::read(fixture.game.path.join(marker))?, b"selected");
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"second");
    let mut reversed = recipe;
    reversed.packages.reverse();
    let backend = fixture.backend("")?;
    let plan = fixture
        .prepare(reversed, Some(&installed), Some(backend.clone()))
        .await?;
    assert!(
        !plan
            .files()
            .iter()
            .any(|file| file.destination.relative == marker)
    );
    let changed = fixture
        .apply_prepared(plan, Some(&installed), Some(backend))
        .await?;
    assert!(!fixture.game.path.join(marker).exists());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"first-m3m");
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&changed))
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn preparation_rejects_tampered_outputs_and_changed_condition_inputs() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let package = fixture.sized_package(8, ENGINE).await?;
    let recipe = fixture.recipe(vec![package]);
    let plan = fixture.prepare(recipe.clone(), None, None).await?;
    let root = plan
        .prepared
        .as_ref()
        .context("missing prepared output")?
        .prepared
        .directory
        .path()
        .to_path_buf();
    fs::write(root.join(ENGINE), b"edited")?;
    assert!(fixture.apply_prepared(plan, None, None).await.is_err());
    assert!(!root.exists());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .is_none()
    );
    let plan = fixture.prepare(recipe.clone(), None, None).await?;
    let root = plan
        .prepared
        .as_ref()
        .context("missing prepared output")?
        .prepared
        .directory
        .path()
        .to_path_buf();
    fs::write(root.join("BioGame/Unexpected.pcc"), b"unexpected")?;
    assert!(fixture.apply_prepared(plan, None, None).await.is_err());
    assert!(!root.exists());
    fs::write(fixture.game.path.join(ENGINE), b"external")?;
    assert!(fixture.prepare(recipe, None, None).await.is_err());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"external");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn cancelled_preparation_discards_staging_without_publishing() -> Result<()> {
    let fixture = Fixture::new(Target::Le2).await?;
    let (_, package) = fixture.retain(b"candidate", "").await?;
    let cancelled = cancel();
    let flag = cancelled.clone();
    let result = inspect_prepared_in(
        fixture.tracker.clone(),
        fixture.destination(None, None),
        fixture.recipe(vec![package]),
        fixture.data.clone(),
        cancelled,
        Arc::new(move |_, _| {
            flag.store(true, Ordering::Release);
        }),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert_eq!(fs::read_dir(fixture.data.join("mele-rebuilds"))?.count(), 0);
    assert!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .is_none()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn apply_rechecks_condition_inputs_outside_the_deployed_files() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let input = "BioGame/CookedPCConsole/Core.pcc";
    let package = fixture.sized_package(10, input).await?;
    let plan = fixture
        .prepare(fixture.recipe(vec![package]), None, None)
        .await?;
    fs::write(fixture.game.path.join(input), b"external!!")?;
    assert!(fixture.apply_prepared(plan, None, None).await.is_err());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert_eq!(fs::read(fixture.game.path.join(input))?, b"external!!");
    assert!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .is_none()
    );
    Ok(())
}
