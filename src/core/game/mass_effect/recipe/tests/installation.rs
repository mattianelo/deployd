use super::*;

impl Fixture {
    pub(super) async fn installer_package(
        &self,
        with_files: bool,
        with_m3m: bool,
        tlk: &[(&str, &str, u8)],
        version: &str,
    ) -> Result<Package> {
        let source = self.temp.path().join(Uuid::new_v4().to_string());
        fs::create_dir_all(&source)?;
        let game = self.recipe(Vec::new()).target;
        let game_name = serde_json::to_value(game)?
            .as_str()
            .context("missing game name")?
            .to_owned();
        let mut manifest = format!(
            "[ModManager]\ncmmver={version}\n[ModInfo]\ngame={game_name}\nmodname=Ordered example\nmodver=1.0\nmoddev=Author\nmoddesc=Example\n"
        );
        if with_files || with_m3m {
            manifest.push_str("[BASEGAME]\nmoddir=.\n");
            if with_files {
                fs::write(source.join("Engine.pcc"), b"first")?;
                manifest.push_str(
                    "newfiles=Engine.pcc\nreplacefiles=BioGame/CookedPCConsole/Engine.pcc\n",
                );
            }
            if with_m3m {
                manifest.push_str("mergemods=Example.m3m\n");
                fs::create_dir(source.join("MergeMods"))?;
                let json = serde_json::json!({"game":game,"files":[{"filename":"Engine.pcc","changes":[
                    {"entryname":"Example.Fn","scriptupdate":{"scriptfilename":"Example.uc","scripttext":"function Fn() {}"}}
                ]}]}).to_string();
                let mut data = b"M3MM\x01".to_vec();
                data.extend_from_slice(&(json.len() as i32 + 1).to_le_bytes());
                data.extend(json.as_bytes());
                data.push(0);
                data.extend_from_slice(&0_i32.to_le_bytes());
                fs::write(source.join("MergeMods/Example.m3m"), data)?;
            }
        }
        if !tlk.is_empty() {
            manifest.push_str("[GAME1_EMBEDDED_TLK]\nusesfeature=true\n");
            fs::create_dir(source.join("GAME1_EMBEDDED_TLK"))?;
            fs::write(
                source.join("GAME1_EMBEDDED_TLK/CombinedTLKMergeData.m3za"),
                crate::core::game::mass_effect::m3za::tests::archive(2, tlk, &["Chosen"], 0)?,
            )?;
        }
        fs::write(source.join("moddesc.ini"), manifest)?;
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

const XML: &str = "<tlkFile><string><id>1</id><data>Example</data></string></tlkFile>";

// @variants: both
#[tokio::test]
async fn le2_and_le3_scripts_rebuild_and_remove() -> Result<()> {
    for game in [Target::Le2, Target::Le3] {
        let fixture = Fixture::new(game).await?;
        let package = fixture.installer_package(false, true, &[], "9.1").await?;
        let recipe = fixture.merge_recipe(vec![package]);
        let backend = fixture.backend("")?;
        let installed = fixture
            .deploy_merges(
                recipe.clone(),
                None,
                backend.clone(),
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original-m3m");
        let rebuilt = fixture
            .deploy_merges(
                recipe.clone(),
                Some(&installed),
                backend.clone(),
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original-m3m");
        let mut disabled = recipe;
        disabled.packages[0].enabled = false;
        fixture
            .deploy_merges(
                disabled,
                Some(&rebuilt),
                backend,
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn replays_files_then_m3m_then_tlk_before_later_package_replacements() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let first = fixture
        .installer_package(true, true, &[("Engine.Example.tlk.xml", XML, 255)], "9.1")
        .await?;
    let (_, last) = fixture.retain(b"last", "").await?;
    let backend = fixture.backend("")?;
    let mut recipe = fixture.merge_recipe(vec![last.clone(), first.clone()]);
    let plan = fixture.inspect(recipe.clone()).await?;
    assert!(
        plan.generated_files()
            .contains("CookedPCConsole/Engine.pcc")
    );
    let installed = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"first-m3m-tlk");
    recipe.packages.reverse();
    let reordered = fixture
        .deploy_merges(
            recipe.clone(),
            Some(&installed),
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"last");
    recipe.packages[1].enabled = false;
    let rebuilt = fixture
        .deploy_merges(
            recipe.clone(),
            Some(&reordered),
            backend,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"first-m3m-tlk");
    let restored = fixture
        .deploy(fixture.recipe(Vec::new()), Some(&rebuilt))
        .await?;
    assert!(restored.files.is_empty());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert!(
        !installed
            .files
            .iter()
            .any(|file| file.relative.ends_with("/Core.pcc"))
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn tlk_choices_follow_manifest_version_and_missing_targets_are_reported() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    for version in ["8.2", "9.1"] {
        let package = fixture
            .installer_package(
                false,
                false,
                &[
                    ("Engine.Example.tlk.xml", XML, 0),
                    ("Absent_LOC_FR.Example.tlk.xml", XML, 255),
                ],
                version,
            )
            .await?;
        let mut recipe = fixture.merge_recipe(vec![package]);
        let inspected = fixture.inspect(recipe.clone()).await?;
        assert_eq!(inspected.skipped_tlk().len(), 1);
        assert_eq!(inspected.skipped_tlk()[0].target, "Absent_LOC_FR.pcc");
        assert_eq!(
            inspected.installation.steps[0].tlk.len(),
            usize::from(version == "8.2")
        );
        recipe.packages[0].options.insert("Chosen".into());
        let inspected = fixture.inspect(recipe).await?;
        assert_eq!(inspected.installation.steps[0].tlk.len(), 1);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn tlk_uses_highest_mounted_available_package_without_requiring_a_baseline_copy() -> Result<()>
{
    let fixture = Fixture::new(Target::Le1).await?;
    let make_dlc = |dlc: &str, mount: i32| -> Result<(PathBuf, PackagePlan)> {
        let source = fixture.temp.path().join(dlc);
        fs::create_dir_all(source.join(dlc).join("CookedPCConsole/NPCs/Other NPCs"))?;
        fs::write(
            source
                .join(dlc)
                .join("CookedPCConsole/NPCs/Other NPCs/Engine.pcc"),
            dlc.as_bytes(),
        )?;
        fs::write(
            source.join(dlc).join("AutoLoad.ini"),
            format!("[ME1DLCMOUNT]\nModMount={mount}"),
        )?;
        fs::write(
            source.join("moddesc.ini"),
            format!(
                "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE1\nmodname=DLC\nmodver=1.0\nmoddev=Author\nmoddesc=Example\n[CUSTOMDLC]\nsourcedirs={dlc}\ndestdirs={dlc}\n"
            ),
        )?;
        let plan = PackagePlan::inspect(&source, None)?;
        Ok((source, plan))
    };
    let mut packages = Vec::new();
    for (dlc, mount) in [("DLC_MOD_High", 50), ("DLC_MOD_Low", 5)] {
        let (source, plan) = make_dlc(dlc, mount)?;
        packages.push(
            sources::retain_in(
                source,
                plan,
                None,
                fixture.data.clone(),
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?
            .selection(),
        );
    }
    packages.push(
        fixture
            .installer_package(false, false, &[("Engine.Example.tlk.xml", XML, 255)], "9.1")
            .await?,
    );
    let mut recipe = fixture.merge_recipe(packages);
    let plan = fixture.inspect(recipe.clone()).await?;
    assert!(plan.merges.originals.is_empty());
    assert_eq!(
        plan.installation.steps[2].tlk[0].target,
        "DLC/DLC_MOD_High/CookedPCConsole/NPCs/Other NPCs/Engine.pcc"
    );
    let installed = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            fixture.backend("")?,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(
        fs::read(
            fixture
                .game
                .path
                .join("BioGame/DLC/DLC_MOD_High/CookedPCConsole/NPCs/Other NPCs/Engine.pcc")
        )?,
        b"DLC_MOD_High-tlk"
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    recipe.packages[0].enabled = false;
    let removed = fixture
        .deploy_merges(
            recipe,
            Some(&installed),
            fixture.backend("")?,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert!(
        !removed
            .files
            .iter()
            .any(|file| file.relative.contains("DLC_MOD_High"))
    );
    assert_eq!(
        fs::read(
            fixture
                .game
                .path
                .join("BioGame/DLC/DLC_MOD_Low/CookedPCConsole/NPCs/Other NPCs/Engine.pcc")
        )?,
        b"DLC_MOD_Low-tlk"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failure_after_m3m_does_not_publish_partial_installation() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let package = fixture
        .installer_package(true, true, &[("Engine.Example.tlk.xml", XML, 255)], "9.1")
        .await?;
    let recipe = fixture.merge_recipe(vec![package]);
    let backend = fixture.backend("if request['operation'] == 'le1-tlk': sys.exit(1)")?;
    assert!(
        fixture
            .deploy_merges(recipe, None, backend, cancel(), Arc::new(|_, _| {}))
            .await
            .is_err()
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .is_none()
    );
    for directory in ["mele-rebuilds", "mele-preparations", "mele-transformations"] {
        assert_eq!(fs::read_dir(fixture.data.join(directory))?.count(), 0);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn target_merges_consume_tlk_transformed_contributions_and_rebuild_without_them() -> Result<()>
{
    let fixture = Fixture::new(Target::Le1).await?;
    let table = fixture.merge_package("DLC_MOD_Test", 5, &["table"]).await?;
    let tlk = fixture
        .installer_package(false, false, &[("Table.Example.tlk.xml", XML, 255)], "9.1")
        .await?;
    let mut recipe = fixture.merge_recipe(vec![table, tlk]);
    let backend = fixture.backend("")?;
    let installed = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    let contribution = fixture
        .game
        .path
        .join("BioGame/DLC/DLC_MOD_Test/CookedPCConsole/Table.pcc");
    assert_eq!(fs::read(&contribution)?, b"table-tlk");
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original-merged"
    );
    recipe.packages[1].enabled = false;
    fixture
        .deploy_merges(
            recipe,
            Some(&installed),
            backend,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(contribution)?, b"table");
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original-merged"
    );
    Ok(())
}
