use std::path::Path;

use sha2::{Digest, Sha256};

use super::*;

fn copy(
    source: &Path,
    destination: &Path,
    originals: &mut BTreeMap<PathBuf, String>,
) -> Result<()> {
    let bytes = fs::read(source)?;
    originals.insert(
        source.to_path_buf(),
        format!("{:x}", Sha256::digest(&bytes)),
    );
    fs::create_dir_all(destination.parent().context("missing corpus directory")?)?;
    fs::write(destination, bytes)?;
    Ok(())
}

fn effective(root: &Path, name: &str, game: Target) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    let base = root.join("BioGame/CookedPCConsole").join(name);
    if base.is_file() {
        candidates.push((0, base));
    }
    for dlc in fs::read_dir(root.join("BioGame/DLC"))? {
        let path = dlc?.path().join("CookedPCConsole").join(name);
        if path.is_file() {
            let mount = path
                .parent()
                .context("missing mount parent")?
                .join("Mount.dlc");
            let bytes = fs::read(&mount)?;
            let offset = if game == Target::Le2 { 12 } else { 16 };
            let priority = i32::from_le_bytes(
                bytes
                    .get(offset..offset + 4)
                    .context("invalid corpus mount")?
                    .try_into()?,
            );
            candidates.push((priority, path));
        }
    }
    candidates.sort();
    candidates
        .pop()
        .map(|(_, path)| path)
        .with_context(|| format!("Missing supplied game package {name}"))
}

fn mount(game: Target, priority: i32, name: &str) -> Vec<u8> {
    let mut bytes = vec![0; if game == Target::Le2 { 44 } else { 108 }];
    let header: &[u32] = if game == Target::Le2 {
        &[684, 168, 65643]
    } else {
        &[1, 685, 205, 196715]
    };
    for (i, value) in header.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    let offset = if game == Target::Le2 { 12 } else { 16 };
    bytes[offset..offset + 4].copy_from_slice(&priority.to_le_bytes());
    if game == Target::Le2 {
        bytes.extend_from_slice(&(-2_i32).to_le_bytes());
        bytes.extend_from_slice(&[b'T', 0, 0, 0]);
        bytes.extend_from_slice(&123_i32.to_le_bytes());
        bytes.extend_from_slice(&((name.len() + 1) as i32).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0_i32.to_le_bytes());
        bytes.extend_from_slice(&123_i32.to_le_bytes());
    } else {
        bytes[24..28].copy_from_slice(&8_u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&123_u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&123_u32.to_le_bytes());
    }
    bytes
}

fn backend() -> Result<helper::Backend> {
    let spec: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../helpers/mele/toolchain.json"
    ))?;
    let sdk = spec["sdk"]["version"].as_str().context("missing SDK")?;
    let root = PathBuf::from("/build/mele/source/Deployd.Mele/bin/Release/net10.0");
    Ok(helper::Backend {
        runtime: PathBuf::from(format!("/build/mele/sdk-{sdk}/dotnet")),
        assembly: root.join("Deployd.Mele.dll"),
        native_library: root.join("libdeployd_oodle.so"),
    })
}

async fn package(
    fixture: &Fixture,
    supplied: &Path,
    game: Target,
    number: i32,
    originals: &mut BTreeMap<PathBuf, String>,
) -> Result<Package> {
    let dlc = format!("DLC_MOD_Corpus{number}");
    let source = fixture.temp.path().join(format!("source-{number}"));
    let cooked = source.join(&dlc).join("CookedPCConsole");
    fs::create_dir_all(&cooked)?;
    fs::write(cooked.join("Mount.dlc"), mount(game, 5000 + number, &dlc))?;
    let game_name = if game == Target::Le2 { "LE2" } else { "LE3" };
    fs::write(
        source.join("moddesc.ini"),
        format!(
            "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame={game_name}\nmodname=Merge corpus {number}\nmodver=1.0\nmoddev=Deployd\nmoddesc=Authored merge fixture\n[CUSTOMDLC]\nsourcedirs={dlc}\ndestdirs={dlc}\n"
        ),
    )?;
    if number == 1 || game == Target::Le3 {
        let (hench, original, image, available, highlight) = if game == Target::Le2 {
            (
                "Vixen",
                "BioH_Vixen_00",
                "BioH_SelectGUI.pcc",
                "GUI_SF_TeamSelect.TeamSelect_I4C",
                "GUI_SF_TeamSelect.TeamSelect_I4F",
            )
        } else {
            (
                "Liara",
                "BioH_Liara_04",
                "SFXHenchImagesLiara_APP01.pcc",
                "GUI_Henchmen_Images_App01.Liara4",
                "GUI_Henchmen_Images_App01.Liara4Glow",
            )
        };
        let name = format!("BioH_{hench}_Deployd{number}");
        copy(
            &effective(supplied, &format!("{original}.pcc"), game)?,
            &cooked.join(format!("{name}.pcc")),
            originals,
        )?;
        if game == Target::Le3 {
            copy(
                &effective(supplied, &format!("{original}_Explore.pcc"), game)?,
                &cooked.join(format!("{name}_Explore.pcc")),
                originals,
            )?;
        }
        if game == Target::Le2 {
            copy(
                &effective(supplied, "BioH_END_Vixen_00.pcc", game)?,
                &cooked.join(format!("BioH_END_{}.pcc", &name[5..])),
                originals,
            )?;
        }
        copy(
            &effective(supplied, image, game)?,
            &cooked.join(format!("SFXHenchImages_{dlc}.pcc")),
            originals,
        )?;
        fs::write(
            cooked.join("SquadmateMergeInfo.sqm"),
            serde_json::to_vec(&serde_json::json!({"game":game_name,"outfits":[{
                "henchname":hench,"henchpackage":name,"availableimage":available,"highlightimage":highlight
            }]}))?,
        )?;
    }
    if game == Target::Le2 {
        for (file, offset) in [("EmailMergeInfo.emm", 0), ("Additional.emm", 1)] {
            fs::write(
                cooked.join(file),
                serde_json::to_vec(
                    &serde_json::json!({"game":"LE2","modName":dlc,"inMemoryBool":123,
                "emails":[{"emailName":format!("Email{number}_{offset}"),"statusPlotInt":900000+number*10+offset,"triggerConditional":"return true;",
                    "titleStrRef":1234,"descStrRef":1235,"readTransition":1236}]}),
                )?,
            )?;
        }
    }
    let plan = PackagePlan::inspect(&source, Some(game))?;
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
#[ignore = "Requires supplied LE2/LE3 game copies, the pinned helper, and disposable staging space"]
async fn deploys_outfits_and_emails_rebuilds_and_removes_generated_dlc() -> Result<()> {
    for game in [Target::Le2, Target::Le3] {
        let folder = if game == Target::Le2 { "ME2" } else { "ME3" };
        eprintln!("Preparing {folder} generated-DLC corpus");
        let supplied = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("modTesting/Mass Effect Legendary Edition (Game)/Game")
            .join(folder);
        let mut originals = BTreeMap::new();
        let mut names = vec!["BioP_Global.pcc"];
        if game == Target::Le2 {
            names.extend(helper::jobs::compiler_bases(game));
            names.extend(super::super::super::merge_dlc::UI_PACKAGES);
            names.extend([
                "BioP_EndGm_StuntHench.pcc",
                "BioD_ZyaVTL_110Jungle.pcc",
                "BioD_Nor_103Messages.pcc",
            ]);
        }
        let fixture = Fixture::with_game(game, |destination| {
            fs::remove_dir_all(destination.join("BioGame/CookedPCConsole"))?;
            fs::create_dir_all(destination.join("BioGame/CookedPCConsole"))?;
            for name in names {
                let path = effective(&supplied, name, game)?;
                let relative = path.strip_prefix(&supplied)?;
                copy(&path, &destination.join(relative), &mut originals)?;
                if relative.starts_with("BioGame/DLC") {
                    let mount = path
                        .parent()
                        .context("missing mount parent")?
                        .join("Mount.dlc");
                    copy(
                        &mount,
                        &destination.join(mount.strip_prefix(&supplied)?),
                        &mut originals,
                    )?;
                }
            }
            copy(
                &supplied.join("Binaries/Win64/oo2core_8_win64.dll"),
                &destination.join("Binaries/Win64/oo2core_8_win64.dll"),
                &mut originals,
            )?;
            Ok(())
        })
        .await?;
        let first = package(&fixture, &supplied, game, 1, &mut originals).await?;
        let second = package(&fixture, &supplied, game, 2, &mut originals).await?;
        let recipe = fixture.merge_recipe(vec![second.clone(), first.clone()]);
        eprintln!("Generating and deploying {folder} combined merges");
        let installed = fixture
            .deploy_merges(
                recipe.clone(),
                None,
                backend()?,
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        let prefix = "BioGame/DLC/DLC_MOD_M3_MERGE/CookedPCConsole/";
        assert!(
            installed
                .files
                .iter()
                .any(|file| file.relative == format!("{prefix}BioP_Global.pcc"))
        );
        assert!(
            !installed
                .files
                .iter()
                .any(|file| file.relative.contains(".merge-ui"))
        );
        eprintln!("Rebuilding {folder} from the same recipe");
        let rebuilt = fixture
            .deploy_merges(
                recipe,
                Some(&installed),
                backend()?,
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        eprintln!("Removing a contributor and regenerating {folder}");
        let reduced = fixture
            .deploy_merges(
                fixture.merge_recipe(vec![first]),
                Some(&rebuilt),
                backend()?,
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        fixture
            .deploy(fixture.recipe(Vec::new()), Some(&reduced))
            .await?;
        for file in &fixture.baseline.files {
            assert_eq!(
                format!(
                    "{:x}",
                    Sha256::digest(fs::read(fixture.game.path.join(&file.relative))?)
                ),
                file.sha256,
                "{}",
                file.relative
            );
        }
        let generated = fixture.game.path.join("BioGame/DLC/DLC_MOD_M3_MERGE");
        assert!(!generated.exists());
        for (path, expected) in originals {
            assert_eq!(format!("{:x}", Sha256::digest(fs::read(path)?)), expected);
        }
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn generated_dlc_failures_preserve_the_previous_deployment() -> Result<()> {
    let fixture = Fixture::with_game(Target::Le3, |root| {
        fs::write(
            root.join("BioGame/CookedPCConsole/BioP_Global.pcc"),
            b"original streaming",
        )?;
        Ok(())
    })
    .await?;
    let source = fixture.temp.path().join("outfit");
    let dlc = "DLC_MOD_Outfit";
    let cooked = source.join(dlc).join("CookedPCConsole");
    fs::create_dir_all(&cooked)?;
    fs::write(
        source.join("moddesc.ini"),
        format!(
            "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame=LE3\nmodname=Outfit\nmodver=1\nmoddev=Deployd\nmoddesc=Fixture\n[CUSTOMDLC]\nsourcedirs={dlc}\ndestdirs={dlc}\n"
        ),
    )?;
    fs::write(cooked.join("Mount.dlc"), mount(Target::Le3, 5000, dlc))?;
    for name in [
        "BioH_Liara_Test.pcc",
        "BioH_Liara_Test_Explore.pcc",
        "SFXHenchImages_DLC_MOD_Outfit.pcc",
    ] {
        fs::write(cooked.join(name), b"authored fixture")?;
    }
    fs::write(cooked.join("SquadmateMergeInfo.sqm"), br#"{"game":"LE3","outfits":[{"henchname":"Liara","henchpackage":"BioH_Liara_Test","availableimage":"Images.Available"}]}"#)?;
    let plan = PackagePlan::inspect(&source, Some(Target::Le3))?;
    let stored = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let recipe = fixture.merge_recipe(vec![stored.selection()]);
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
    let generated = fixture
        .game
        .path
        .join("BioGame/DLC/DLC_MOD_M3_MERGE/CookedPCConsole/BioP_Global.pcc");
    assert_eq!(fs::read(&generated)?, b"generated-merge");
    let broken = fixture.backend("sys.exit(1)")?;
    assert!(
        fixture
            .deploy_merges(
                recipe.clone(),
                Some(&installed),
                broken,
                cancel(),
                Arc::new(|_, _| {})
            )
            .await
            .is_err()
    );
    let backend = fixture.backend("")?;
    let cancelled = cancel();
    let signal = cancelled.clone();
    assert!(
        fixture
            .deploy_merges(
                recipe.clone(),
                Some(&installed),
                backend.clone(),
                cancelled,
                Arc::new(move |done, total| {
                    if done * 2 > total {
                        signal.store(true, Ordering::Release);
                    }
                })
            )
            .await
            .is_err()
    );
    sqlx::query("CREATE TRIGGER reject_recipe BEFORE UPDATE ON mele_recipes BEGIN SELECT RAISE(ABORT, 'fixture failure'); END").execute(&fixture.tracker.pool).await?;
    assert!(
        fixture
            .deploy_merges(
                recipe,
                Some(&installed),
                backend,
                cancel(),
                Arc::new(|_, _| {})
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read(&generated)?, b"generated-merge");
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
    sqlx::query("DROP TRIGGER reject_recipe")
        .execute(&fixture.tracker.pool)
        .await?;
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&installed))
        .await?;
    assert!(
        !fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_M3_MERGE")
            .exists()
    );
    assert_eq!(
        fs::read(
            fixture
                .game
                .path
                .join("BioGame/CookedPCConsole/BioP_Global.pcc")
        )?,
        b"original streaming"
    );
    Ok(())
}
