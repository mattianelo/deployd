use super::*;

async fn retain(fixture: &Fixture, source: PathBuf) -> Result<Package> {
    let plan = PackagePlan::inspect(&source, None)?;
    let options = plan.default_options();
    let stored = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let mut selection = stored.selection();
    selection.options = options;
    Ok(selection)
}

// @variants: both
#[tokio::test]
async fn advanced_basegame_choices_merge_multilists_and_rebuild() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let source = fixture.temp.path().join("advanced");
    fs::create_dir_all(source.join("MergeMods"))?;
    fs::create_dir_all(source.join("Extra"))?;
    fs::write(source.join("Default.pcc"), b"default")?;
    fs::write(source.join("Alternate.pcc"), b"alternate")?;
    fs::write(source.join("Extra/Added.pcc"), b"added")?;
    fs::write(source.join("Extra/Omitted.pcc"), b"omitted")?;
    let json = serde_json::json!({"game":"LE1","files":[{"filename":"Engine.pcc","changes":[{"entryname":"Example.Fn","scriptupdate":{"scriptfilename":"Example.uc","scripttext":"function Fn() {}"}}]}]}).to_string();
    let mut merge = b"M3MM\x01".to_vec();
    merge.extend_from_slice(&(json.len() as i32 + 1).to_le_bytes());
    merge.extend(json.as_bytes());
    merge.push(0);
    merge.extend_from_slice(&0_i32.to_le_bytes());
    fs::write(source.join("MergeMods/Example.m3m"), merge)?;
    fs::write(
        source.join("moddesc.ini"),
        r#"[ModManager]
cmmver=9.2
minbuild=137
[ModInfo]
game=LE1
modname=Advanced
modver=2.0
moddev=Deployd
moddesc=Fixture
sortalternates=false
[BASEGAME]
moddir=.
newfiles=Default.pcc
replacefiles=BioGame/CookedPCConsole/Engine.pcc
multilist1=Added.pcc;Omitted.pcc
multilist2=Omitted.pcc
altfiles=((FriendlyName=Alternate,Condition=COND_MANUAL,CheckedByDefault=true,ModOperation=OP_SUBSTITUTE,ModFile=BioGame/CookedPCConsole/Engine.pcc,AltFile=Alternate.pcc),(FriendlyName=Merge,Condition=COND_MANUAL,CheckedByDefault=true,ModOperation=OP_APPLY_MERGEMODS,MergeFiles=Example.m3m),(FriendlyName=Add,Condition=COND_ALWAYS,ModOperation=OP_APPLY_MULTILISTFILES,MultiListId=1,MultiListRootPath=Extra,MultiListTargetPath=BioGame/CookedPCConsole),(FriendlyName=Exclude,Condition=COND_ALWAYS,ModOperation=OP_NOINSTALL_MULTILISTFILES,MultiListId=2,MultiListTargetPath=BioGame/CookedPCConsole))
"#,
    )?;
    let mut package = retain(&fixture, source.clone()).await?;
    let backend = fixture.backend("")?;
    let first = fixture
        .deploy_merges(
            fixture.merge_recipe(vec![package.clone()]),
            None,
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"alternate-m3m");
    assert_eq!(
        fs::read(fixture.game.path.join("BioGame/CookedPCConsole/Added.pcc"))?,
        b"added"
    );
    assert!(
        !fixture
            .game
            .path
            .join("BioGame/CookedPCConsole/Omitted.pcc")
            .exists()
    );
    package.options.clear();
    let second = fixture
        .deploy_merges(
            fixture.merge_recipe(vec![package]),
            Some(&first),
            backend,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"default");
    assert_eq!(fs::read(source.join("Alternate.pcc"))?, b"alternate");
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&second))
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn advanced_localization_follows_its_dlc_and_restores_independently() -> Result<()> {
    for target in [Target::Le2, Target::Le3] {
        let fixture = Fixture::new(target).await?;
        let (_, content) = fixture.retain(b"content", "").await?;
        let source = fixture.temp.path().join("localization");
        fs::create_dir_all(source.join("Languages"))?;
        fs::write(source.join("Languages/DLC_MOD_Test_FRA.tlk"), b"localized")?;
        let game = if target == Target::Le2 { "LE2" } else { "LE3" };
        fs::write(
            source.join("moddesc.ini"),
            format!(
                "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame={game}\nmodname=Localization\nmodver=1.0\nmoddev=Deployd\nmoddesc=Fixture\n[LOCALIZATION]\nfiles=Languages/DLC_MOD_Test_FRA.tlk\ndlcname=DLC_MOD_Test\n"
            ),
        )?;
        let localization = retain(&fixture, source).await?;
        assert!(
            fixture
                .inspect(fixture.recipe(vec![localization.clone()]))
                .await
                .is_err()
        );
        let first = fixture
            .deploy(fixture.recipe(vec![content.clone(), localization]), None)
            .await?;
        let path = fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_Test/CookedPCConsole/DLC_MOD_Test_FRA.tlk");
        assert_eq!(fs::read(&path)?, b"localized");
        fixture
            .deploy(fixture.recipe(vec![content]), Some(&first))
            .await?;
        assert!(!path.exists());
    }
    Ok(())
}
