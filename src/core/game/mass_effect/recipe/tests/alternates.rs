use super::*;

async fn retain_folder(fixture: &Fixture, source: PathBuf) -> Result<Package> {
    let plan = PackagePlan::inspect(&source, None)?;
    let stored = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    Ok(stored.selection())
}

// @variants: both
#[tokio::test]
async fn alternates_rebuild_with_order_dependencies_and_saved_manual_choices() -> Result<()> {
    for target in [Target::Le1, Target::Le2, Target::Le3] {
        let fixture = Fixture::new(target).await?;
        let (_, dependency) = fixture.retain(b"dependency", "").await?;
        let source = fixture.temp.path().join("options");
        let wrapper = source.join("wrapper");
        fs::create_dir_all(wrapper.join("Source/CookedPCConsole"))?;
        fs::create_dir_all(wrapper.join("Optional/CookedPCConsole"))?;
        fs::write(wrapper.join("Source/CookedPCConsole/A.pcc"), b"default")?;
        fs::write(wrapper.join("Source/CookedPCConsole/B.pcc"), b"excluded")?;
        fs::write(wrapper.join("replacement.pcc"), b"compatible")?;
        fs::write(wrapper.join("Optional/CookedPCConsole/C.pcc"), b"manual")?;
        let game = match target {
            Target::Le1 => "LE1",
            Target::Le2 => "LE2",
            Target::Le3 => "LE3",
        };
        fs::write(
            wrapper.join("moddesc.ini"),
            format!(
                r#"[ModManager]
cmmver=9.1
[ModInfo]
game={game}
modname=Alternates
modver=1.0
moddev=Deployd
moddesc=Fixture
[CUSTOMDLC]
sourcedirs=Source
destdirs=DLC_MOD_Options
altfiles=((FriendlyName=Compatibility,Condition=COND_DLC_PRESENT,ConditionalDLC=DLC_MOD_Test,ModOperation=OP_SUBSTITUTE,ModFile=DLC_MOD_Options/CookedPCConsole/A.pcc,AltFile=replacement.pcc),(FriendlyName=Exclude,Condition=COND_ALWAYS,ModOperation=OP_NOINSTALL,ModFile=DLC_MOD_Options/CookedPCConsole/B.pcc),(FriendlyName=Addition,Condition=COND_DLC_PRESENT,ConditionalDLC=DLC_MOD_Test,ModOperation=OP_INSTALL,ModFile=DLC_MOD_Options/CookedPCConsole/D.pcc,AltFile=replacement.pcc))
altdlc=((FriendlyName=Optional,Condition=COND_MANUAL,CheckedByDefault=true,ModOperation=OP_ADD_CUSTOMDLC,ModAltDLC=Optional,ModDestDLC=DLC_MOD_Optional))
"#
            ),
        )?;
        let plan = PackagePlan::inspect(&source, None)?;
        let mut options = retain_folder(&fixture, source.clone()).await?;
        options.options = plan.default_options();
        let recipe = fixture.recipe(vec![dependency.clone(), options.clone()]);
        let first = fixture.deploy(recipe.clone(), None).await?;
        let output = fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_Options/CookedPCConsole/A.pcc");
        let optional = fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_Optional/CookedPCConsole/C.pcc");
        let addition = fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_Options/CookedPCConsole/D.pcc");
        assert_eq!(fs::read(&addition)?, b"compatible");
        assert_eq!(fs::read(&output)?, b"compatible");
        assert!(optional.exists());
        assert!(
            !fixture
                .game
                .path
                .join("BioGame/DLC/DLC_MOD_Options/CookedPCConsole/B.pcc")
                .exists()
        );
        assert_eq!(
            fs::read(wrapper.join("Source/CookedPCConsole/A.pcc"))?,
            b"default"
        );
        let saved = fixture
            .tracker
            .mele_recipe(&fixture.game.id, &fixture.profile)
            .await?
            .context("missing recipe")?;
        assert_eq!(saved.packages[1].options, options.options);
        let rebuilt = fixture.deploy(saved, Some(&first)).await?;
        assert_eq!(rebuilt.files, first.files);
        let reversed = fixture.recipe(vec![options.clone(), dependency.clone()]);
        let reordered = fixture.deploy(reversed, Some(&rebuilt)).await?;
        assert_eq!(fs::read(&output)?, b"default");
        let mut disabled = recipe;
        disabled.packages[0].enabled = false;
        disabled.packages[1].options.clear();
        let removed = fixture.deploy(disabled, Some(&reordered)).await?;
        assert_eq!(fs::read(&output)?, b"default");
        assert!(!optional.exists());
        assert!(!addition.exists());
        fixture
            .deploy(fixture.recipe(Vec::new()), Some(&removed))
            .await?;
        assert!(!output.exists());
        assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn grouped_multilists_rebuild_and_exclude_files_added_by_other_options() -> Result<()> {
    let fixture = Fixture::new(Target::Le2).await?;
    let (source, _) = fixture.package(b"base", "")?;
    fs::create_dir_all(source.join("Extra/nested"))?;
    fs::write(source.join("Extra/nested/Added.pcc"), b"added")?;
    fs::write(source.join("Extra/nested/Excluded.pcc"), b"excluded")?;
    let manifest = source.join("moddesc.ini");
    let text = fs::read_to_string(&manifest)?;
    fs::write(&manifest, text.replace("[BASEGAME]", r#"multilist1=nested/Added.pcc;nested/Excluded.pcc
multilist2=Excluded.pcc
altdlc=((FriendlyName=Expanded,Condition=COND_MANUAL,OptionGroup=Content,CheckedByDefault=true,ModOperation=OP_ADD_MULTILISTFILES_TO_CUSTOMDLC,MultiListId=1,MultiListRootPath=Extra,ModDestDLC=DLC_MOD_Test/CookedPCConsole,FlattenMultiListOutput=true),(FriendlyName=Standard,Condition=COND_MANUAL,OptionGroup=Content,ModOperation=OP_NOTHING))
altfiles=((FriendlyName=Exclusions,Condition=COND_ALWAYS,ModOperation=OP_NOINSTALL_MULTILISTFILES,MultiListId=2,MultiListTargetPath=DLC_MOD_Test/CookedPCConsole))
[BASEGAME]"#))?;
    let plan = PackagePlan::inspect(&source, None)?;
    let mut package = retain_folder(&fixture, source.clone()).await?;
    package.options = plan.default_options();
    let installed = fixture
        .deploy(fixture.recipe(vec![package.clone()]), None)
        .await?;
    let added = fixture
        .game
        .path
        .join("BioGame/DLC/DLC_MOD_Test/CookedPCConsole/Added.pcc");
    assert_eq!(fs::read(&added)?, b"added");
    assert!(
        !fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_Test/CookedPCConsole/Excluded.pcc")
            .exists()
    );
    let saved = fixture
        .tracker
        .mele_recipe(&fixture.game.id, &fixture.profile)
        .await?
        .context("missing recipe")?;
    let rebuilt = fixture.deploy(saved, Some(&installed)).await?;
    assert_eq!(rebuilt.files, installed.files);
    package.options = plan
        .manifest
        .alternates
        .iter()
        .filter(|alt| alt.name == "Standard")
        .map(|alt| alt.key.clone())
        .collect();
    let changed = fixture
        .deploy(fixture.recipe(vec![package.clone()]), Some(&rebuilt))
        .await?;
    assert!(!added.exists());
    package.options.clear();
    assert!(
        fixture
            .inspect(fixture.recipe(vec![package.clone()]))
            .await
            .is_err()
    );
    package.options = plan.option_keys();
    assert!(
        fixture
            .inspect(fixture.recipe(vec![package]))
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(source.join("Extra/nested/Excluded.pcc"))?,
        b"excluded"
    );
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&changed))
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn dependent_options_rebuild_from_saved_user_choices() -> Result<()> {
    let fixture = Fixture::new(Target::Le3).await?;
    let (source, _) = fixture.package(b"base", "")?;
    fs::write(source.join("patched.pcc"), b"patch")?;
    let manifest = source.join("moddesc.ini");
    let text = fs::read_to_string(&manifest)?.replace("cmmver=9.1", "cmmver=9.2");
    fs::write(&manifest, text.replace("[BASEGAME]", r#"altfiles=((FriendlyName=Patch,Condition=COND_MANUAL,OptionKey=Patch,DependsOnKeys=+Gate,DependsOnMetAction=ACTION_DISALLOW_SELECT_CHECKED,DependsOnNotMetAction=ACTION_DISALLOW_SELECT,ModOperation=OP_SUBSTITUTE,ModFile=DLC_MOD_Test/CookedPCConsole/Test.pcc,AltFile=patched.pcc),(FriendlyName=Gate,OptionKey=Gate,Condition=COND_MANUAL,ModOperation=OP_NOTHING))
[BASEGAME]"#))?;
    let plan = PackagePlan::inspect(&source, None)?;
    let mut package = retain_folder(&fixture, source.clone()).await?;
    package.options = plan
        .manifest
        .alternates
        .iter()
        .filter(|alt| alt.name == "Gate")
        .map(|alt| alt.key.clone())
        .collect();
    let installed = fixture
        .deploy(fixture.recipe(vec![package.clone()]), None)
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(DLC))?, b"patch");
    let saved = fixture
        .tracker
        .mele_recipe(&fixture.game.id, &fixture.profile)
        .await?
        .context("missing recipe")?;
    assert_eq!(saved.packages[0].options, package.options);
    let rebuilt = fixture.deploy(saved, Some(&installed)).await?;
    assert_eq!(installed.files, rebuilt.files);
    package.options.clear();
    let disabled = fixture
        .deploy(fixture.recipe(vec![package]), Some(&rebuilt))
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(DLC))?, b"base");
    assert_eq!(fs::read(source.join("patched.pcc"))?, b"patch");
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&disabled))
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn conditional_tlk_options_rebuild_when_preceding_dlc_changes() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let source = fixture.temp.path().join("conditional-tlk");
    fs::create_dir_all(source.join("GAME1_EMBEDDED_TLK"))?;
    let xml = "<tlkFile><string><id>1</id><data>Example</data></string></tlkFile>";
    fs::write(
        source.join("GAME1_EMBEDDED_TLK/CombinedTLKMergeData.m3za"),
        crate::core::game::mass_effect::m3za::tests::archive(
            2,
            &[("Engine.Example.tlk.xml", xml, 0)],
            &["Chosen"],
            0,
        )?,
    )?;
    let manifest = "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame=LE1\nmodname=Conditional text\nmodver=1.0\nmoddev=Deployd\nmoddesc=Fixture\n[GAME1_EMBEDDED_TLK]\nusesfeature=true\n[CUSTOMDLC]\naltdlc=((FriendlyName=Compatibility text,Condition=COND_DLC_PRESENT,ConditionalDLC=DLC_MOD_Test,ModOperation=OP_ENABLE_TLKMERGE_OPTIONKEY,LE1TLKOptionKey=Chosen))\n";
    fs::write(source.join("moddesc.ini"), manifest)?;
    let plan = PackagePlan::inspect(&source, None)?;
    assert!(plan.option_keys().is_empty());
    fs::write(
        source.join("moddesc.ini"),
        manifest.replace("cmmver=9.2", "cmmver=8.2"),
    )?;
    assert!(PackagePlan::inspect(&source, None).is_err());
    fs::write(source.join("moddesc.ini"), manifest)?;
    let package = retain_folder(&fixture, source.clone()).await?;
    let (_, dependency) = fixture.retain(b"base", "").await?;
    let backend = fixture.backend("")?;
    let mut recipe = fixture.merge_recipe(vec![dependency, package]);
    let first = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"base-tlk");
    recipe.packages[0].enabled = false;
    let removed = fixture
        .deploy_merges(
            recipe.clone(),
            Some(&first),
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    recipe.packages[0].enabled = true;
    fixture
        .deploy_merges(
            recipe,
            Some(&removed),
            backend,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"base-tlk");
    fs::write(
        source.join("moddesc.ini"),
        manifest.replace("LE1TLKOptionKey=Chosen", "LE1TLKOptionKey=Missing"),
    )?;
    assert!(PackagePlan::inspect(&source, None).is_err());
    Ok(())
}
