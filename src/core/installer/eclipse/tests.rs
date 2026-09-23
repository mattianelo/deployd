use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use super::*;
use crate::core::installer::{PrepareResult, prepare_mod};
use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameEngine};

fn write_zip(path: &Path, files: &[(&str, &[u8])]) -> Result<()> {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path)?);
    for (name, bytes) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(bytes)?;
    }
    zip.finish()?;
    Ok(())
}

struct Fixture {
    temp: tempfile::TempDir,
    tracker: Tracker,
    game: Game,
    roots: Vec<DazipSource>,
    files: Vec<(PathBuf, PathBuf)>,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("staging");
        let dazip = root.join("addon.dazip");
        std::fs::create_dir_all(dazip.join("AddIns/example"))?;
        std::fs::create_dir_all(dazip.join("packages/core/override"))?;
        std::fs::create_dir_all(root.join("override"))?;
        std::fs::write(
            dazip.join("AddIns/example/manifest.xml"),
            b"<AddInItem UID=\"example\"/>",
        )?;
        std::fs::write(
            dazip.join("packages/core/override/shared.gda"),
            b"addin resource",
        )?;
        std::fs::write(root.join("override/shared.gda"), b"loose resource")?;
        let files = [
            "addon.dazip/AddIns/example/manifest.xml",
            "addon.dazip/packages/core/override/shared.gda",
            "override/shared.gda",
        ]
        .into_iter()
        .map(|path| (root.join(path), PathBuf::from(path)))
        .collect();
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let game = Game {
            id: "dragon-age-origins".into(),
            title: "DAO".into(),
            path: temp.path().join("game"),
            data_subdir: "Documents/BioWare/Dragon Age".into(),
            engine: GameEngine::Eclipse,
            wine_prefix: None,
        };
        Ok(Self {
            temp,
            tracker,
            game,
            roots: vec![DazipSource {
                root: dazip,
                key: "dazip:example".into(),
            }],
            files,
        })
    }

    async fn install(
        &self,
        replacing: Option<&str>,
        merging: bool,
        excluded: &HashSet<String>,
    ) -> Result<AddResult> {
        super::add_components(AddModRequest {
            merging,
            replacing,
            dazip_sources: &self.roots,
            file_list: self.files.clone(),
            game: &self.game,
            mod_name: "Mixed archive",
            tracker: &self.tracker,
            cache_root: &self.temp.path().join("cache"),
            nexus_ids: None,
            archive_hash: Some("same-source".into()),
            archive_path: Some("archive.zip".into()),
            file_targets: Default::default(),
            stripped_wrapper: None,
            excluded_files: excluded,
            on_progress: None,
        })
        .await
    }
}

// @variants: appimage, snap
#[tokio::test]
async fn reordered_dao_components_persist_only_to_the_selected_profile() -> Result<()> {
    let fixture = Fixture::new().await?;
    let profile = fixture
        .tracker
        .ensure_default_profile(&fixture.game.id)
        .await?;
    let result = fixture.install(None, false, &HashSet::new()).await?;
    let other = fixture
        .tracker
        .create_profile(&fixture.game.id, "Other")
        .await?;
    fixture
        .tracker
        .save_to_profile(&other, &fixture.game.id)
        .await?;
    let ids = vec![
        result.additional_mods[0].id.clone(),
        result.mod_entry.id.clone(),
    ];
    fixture
        .tracker
        .set_eclipse_order(&fixture.game.id, &profile.id, &ids)
        .await?;
    assert_eq!(
        fixture
            .tracker
            .list_mods(&fixture.game.id)
            .await?
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        ids
    );
    let saved: Vec<String> =
        sqlx::query_scalar("SELECT mod_id FROM profile_mods WHERE profile_id=? ORDER BY priority")
            .bind(&profile.id)
            .fetch_all(&fixture.tracker.pool)
            .await?;
    assert_eq!(saved, ids);
    let saved_other: Vec<String> =
        sqlx::query_scalar("SELECT mod_id FROM profile_mods WHERE profile_id=? ORDER BY priority")
            .bind(&other)
            .fetch_all(&fixture.tracker.pool)
            .await?;
    assert_eq!(saved_other, vec![ids[1].clone(), ids[0].clone()]);
    assert!(
        fixture
            .tracker
            .set_eclipse_order(
                &fixture.game.id,
                &profile.id,
                &[ids[0].clone(), ids[0].clone()]
            )
            .await
            .is_err()
    );
    sqlx::query("CREATE TRIGGER fail_dao_order BEFORE UPDATE ON profile_mods BEGIN SELECT RAISE(ABORT,'injected failure'); END")
        .execute(&fixture.tracker.pool).await?;
    let reverse = vec![ids[1].clone(), ids[0].clone()];
    assert!(
        fixture
            .tracker
            .set_eclipse_order(&fixture.game.id, &profile.id, &reverse)
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .tracker
            .list_mods(&fixture.game.id)
            .await?
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        ids
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn addin_companion_documents_do_not_create_a_phantom_override_component() -> Result<()> {
    let mut fixture = Fixture::new().await?;
    fixture
        .files
        .retain(|(source, _)| source.starts_with(&fixture.roots[0].root));
    for name in [
        "QUDAO Fixpack v3.5 Readme.txt",
        "QUDAO Fixpack v3.5 Affected Files List.txt",
        "UserManifest_example.xml",
    ] {
        let source = fixture.temp.path().join(name);
        std::fs::write(&source, b"companion metadata")?;
        fixture.files.push((source, name.into()));
    }
    let result = fixture.install(None, false, &HashSet::new()).await?;
    assert!(result.additional_mods.is_empty());
    assert!(
        fixture
            .tracker
            .eclipse_override_ids(&fixture.game.id)
            .await?
            .is_empty()
    );
    assert_eq!(
        fixture
            .tracker
            .get_mod_files(&result.mod_entry.id)
            .await?
            .len(),
        5
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn splits_loose_overrides_without_detaching_dazip_resources() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = fixture.install(None, false, &HashSet::new()).await?;
    assert_eq!(result.additional_mods.len(), 1);
    let override_ids = fixture
        .tracker
        .eclipse_override_ids(&fixture.game.id)
        .await?;
    assert_eq!(override_ids.len(), 1);
    assert!(!override_ids.contains(&result.mod_entry.id));
    let addin = fixture.tracker.get_mod_files(&result.mod_entry.id).await?;
    assert_eq!(addin.len(), 2);
    let loose = fixture
        .tracker
        .get_mod_files(&result.additional_mods[0].id)
        .await?;
    assert_eq!(loose.len(), 1);
    let addin_resource = addin
        .iter()
        .find(|file| file.game_rel_lowercase.ends_with("shared.gda"))
        .context("Missing DAZIP resource")?;
    assert_eq!(
        loose[0].game_rel_lowercase,
        addin_resource.game_rel_lowercase
    );
    assert_eq!(std::fs::read(&loose[0].cache_path)?, b"loose resource");
    assert_eq!(
        std::fs::read(&addin_resource.cache_path)?,
        b"addin resource"
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn reinstalling_one_component_preserves_its_disabled_state_and_sibling() -> Result<()> {
    let fixture = Fixture::new().await?;
    let result = fixture.install(None, false, &HashSet::new()).await?;
    let override_id = &result.additional_mods[0].id;
    fixture.tracker.toggle_mod(override_id, false).await?;
    let reinstalled = fixture
        .install(Some(override_id), false, &HashSet::new())
        .await?;
    assert!(reinstalled.additional_mods.is_empty());
    assert!(!reinstalled.mod_entry.enabled);
    let mods = fixture.tracker.list_mods(&fixture.game.id).await?;
    assert_eq!(mods.len(), 2);
    assert!(mods.iter().any(|entry| entry.id == result.mod_entry.id));
    assert!(!mods.iter().any(|entry| entry.id == *override_id));
    assert_eq!(
        fixture
            .tracker
            .get_mod_files(&result.mod_entry.id)
            .await?
            .len(),
        2
    );
    fixture
        .tracker
        .remove_eclipse_mod(&reinstalled.mod_entry.id)
        .await?;
    assert!(
        fixture
            .tracker
            .eclipse_component(&reinstalled.mod_entry.id)
            .await?
            .is_none()
    );
    assert_eq!(fixture.tracker.list_mods(&fixture.game.id).await?.len(), 1);
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn excluded_components_stay_absent_and_failed_reinstall_preserves_existing_mods() -> Result<()>
{
    let fixture = Fixture::new().await?;
    let excluded = HashSet::from(["override/shared.gda".into()]);
    let result = fixture.install(None, false, &excluded).await?;
    assert!(result.additional_mods.is_empty());
    assert!(
        fixture
            .tracker
            .eclipse_override_ids(&fixture.game.id)
            .await?
            .is_empty()
    );
    let excluded = fixture
        .files
        .iter()
        .map(|(_, path)| path.to_string_lossy().into_owned())
        .collect();
    assert!(
        fixture
            .install(Some(&result.mod_entry.id), false, &excluded)
            .await
            .is_err()
    );
    assert_eq!(fixture.tracker.list_mods(&fixture.game.id).await?.len(), 1);
    assert_eq!(
        fixture
            .tracker
            .get_mod_files(&result.mod_entry.id)
            .await?
            .len(),
        2
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn nested_dazip_extraction_preserves_colliding_loose_sources_and_provenance() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let nested = temp.path().join("addon.dazip");
    write_zip(
        &nested,
        &[
            ("Manifest.xml", b"<AddInItem UID=\"addon\"/>"),
            ("Contents/packages/core/override/shared.gda", b"addin"),
        ],
    )?;
    let bytes = std::fs::read(nested)?;
    let archive = temp.path().join("mixed.zip");
    write_zip(
        &archive,
        &[
            ("addon.dazip", &bytes),
            ("packages/core/override/shared.gda", b"loose"),
        ],
    )?;
    let PrepareResult::Normal {
        file_list,
        dazip_sources,
        tmp_dir,
        ..
    } = prepare_mod(&archive, None, None, None).await?
    else {
        anyhow::bail!("Unexpected installer");
    };
    assert_eq!(dazip_sources.len(), 1);
    assert_eq!(dazip_sources[0].key, "dazip:addon");
    assert_eq!(
        std::fs::read(tmp_dir.path().join("packages/core/override/shared.gda"))?,
        b"loose"
    );
    assert_eq!(
        std::fs::read(
            dazip_sources[0]
                .root
                .join("packages/core/override/shared.gda")
        )?,
        b"addin"
    );
    assert_eq!(file_list.len(), 3);
    assert!(
        file_list
            .iter()
            .all(|(_, path)| path != Path::new("addon.dazip/Manifest.xml"))
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn merging_mixed_content_keeps_dazips_separate_from_existing_overrides() -> Result<()> {
    let mut fixture = Fixture::new().await?;
    let original = fixture.files.clone();
    fixture
        .files
        .retain(|(_, path)| path.starts_with("override"));
    let result = fixture.install(None, false, &HashSet::new()).await?;
    fixture.files = original;
    let merged = fixture
        .install(Some(&result.mod_entry.id), true, &HashSet::new())
        .await?;
    assert_eq!(merged.additional_mods.len(), 1);
    assert_eq!(fixture.tracker.list_mods(&fixture.game.id).await?.len(), 2);
    assert_eq!(
        fixture
            .tracker
            .eclipse_override_ids(&fixture.game.id)
            .await?
            .len(),
        1
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn disabling_and_reinstalling_overrides_preserves_independent_profile_states() -> Result<()> {
    let fixture = Fixture::new().await?;
    let active = fixture
        .tracker
        .create_profile(&fixture.game.id, "Active")
        .await?;
    fixture
        .tracker
        .switch_profile(&fixture.game.id, &active)
        .await?;
    let result = fixture.install(None, false, &HashSet::new()).await?;
    let other = fixture
        .tracker
        .create_profile(&fixture.game.id, "Other")
        .await?;
    fixture
        .tracker
        .save_to_profile(&other, &fixture.game.id)
        .await?;
    let override_id = &result.additional_mods[0].id;
    fixture
        .tracker
        .set_eclipse_enabled(std::slice::from_ref(override_id), false, Some(&active))
        .await?;
    let reinstalled = fixture
        .install(Some(override_id), false, &HashSet::new())
        .await?;
    fixture
        .tracker
        .switch_profile(&fixture.game.id, &other)
        .await?;
    assert!(
        fixture
            .tracker
            .list_mods(&fixture.game.id)
            .await?
            .iter()
            .all(|entry| entry.enabled)
    );
    fixture
        .tracker
        .switch_profile(&fixture.game.id, &active)
        .await?;
    let mods = fixture.tracker.list_mods(&fixture.game.id).await?;
    assert!(
        mods.iter()
            .any(|entry| entry.id == result.mod_entry.id && entry.enabled)
    );
    assert!(
        mods.iter()
            .any(|entry| entry.id == reinstalled.mod_entry.id && !entry.enabled)
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn failing_component_commit_rolls_back_every_new_entry_and_keeps_replacement() -> Result<()> {
    let fixture = Fixture::new().await?;
    let original = fixture.install(None, false, &HashSet::new()).await?;
    let installs: Vec<_> = ["override", "invalid-kind"]
        .into_iter()
        .map(|kind| {
            let mut entry = original.mod_entry.clone();
            entry.id = Uuid::new_v4().to_string();
            EclipseInstall {
                entry,
                component: EclipseComponent {
                    kind: kind.into(),
                    source_key: kind.into(),
                },
                files: Vec::new(),
            }
        })
        .collect();
    assert!(
        fixture
            .tracker
            .save_eclipse_install(&installs, Some(&original.mod_entry.id))
            .await
            .is_err()
    );
    let entries = fixture.tracker.list_mods(&fixture.game.id).await?;
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .any(|entry| entry.id == original.mod_entry.id)
    );
    assert_eq!(
        fixture
            .tracker
            .get_mod_files(&original.mod_entry.id)
            .await?
            .len(),
        2
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn legacy_dazips_with_override_resources_are_not_duplicated_in_the_override_panel()
-> Result<()> {
    let fixture = Fixture::new().await?;
    let installed = fixture.install(None, false, &HashSet::new()).await?;
    let addin_files = fixture
        .tracker
        .get_mod_files(&installed.mod_entry.id)
        .await?;
    let mut legacy = installed.mod_entry.clone();
    legacy.id = "legacy".into();
    fixture.tracker.insert_mod(&legacy).await?;
    let files: Vec<_> = addin_files
        .into_iter()
        .map(|mut file| {
            file.mod_id = legacy.id.clone();
            file
        })
        .collect();
    fixture.tracker.record_files(&files).await?;
    let ids = fixture
        .tracker
        .eclipse_override_ids(&fixture.game.id)
        .await?;
    assert_eq!(ids.len(), 1);
    assert!(!ids.contains(&legacy.id));
    assert!(
        fixture
            .tracker
            .eclipse_override_ids("another-game")
            .await?
            .is_empty()
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn deselecting_loose_resource_does_not_deselect_dazip_resource_at_same_destination()
-> Result<()> {
    let mut fixture = Fixture::new().await?;
    fixture.files[2].1 = PathBuf::from("packages/core/override/shared.gda");
    let result = fixture
        .install(
            None,
            false,
            &HashSet::from(["packages/core/override/shared.gda".into()]),
        )
        .await?;
    assert!(result.additional_mods.is_empty());
    assert_eq!(
        fixture
            .tracker
            .get_mod_files(&result.mod_entry.id)
            .await?
            .len(),
        2
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn dazip_identity_survives_archive_filename_changes() -> Result<()> {
    let mut fixture = Fixture::new().await?;
    let installed = fixture.install(None, false, &HashSet::new()).await?;
    let old_root = fixture.roots[0].root.clone();
    let new_root = old_root.with_file_name("renamed-new-version.dazip");
    std::fs::rename(&old_root, &new_root)?;
    for (source, destination) in &mut fixture.files {
        if let Ok(relative) = source.strip_prefix(&old_root) {
            let relative = relative.to_path_buf();
            *source = new_root.join(&relative);
            *destination = PathBuf::from("renamed-new-version.dazip").join(relative);
        }
    }
    fixture.roots[0].root = new_root;
    let replacement = fixture
        .install(Some(&installed.mod_entry.id), false, &HashSet::new())
        .await?;
    assert!(replacement.additional_mods.is_empty());
    assert_eq!(
        fixture
            .tracker
            .get_mod_files(&replacement.mod_entry.id)
            .await?
            .len(),
        2
    );
    assert_eq!(fixture.tracker.list_mods(&fixture.game.id).await?.len(), 2);
    assert_eq!(
        fixture
            .tracker
            .eclipse_override_ids(&fixture.game.id)
            .await?,
        HashSet::from([installed.additional_mods[0].id.clone()])
    );
    Ok(())
}

// @variants: appimage, snap
#[tokio::test]
async fn duplicate_selected_dazip_identities_require_choosing_one_version() -> Result<()> {
    let mut fixture = Fixture::new().await?;
    let alternative = fixture.temp.path().join("alternative.dazip");
    std::fs::create_dir_all(alternative.join("AddIns/example"))?;
    let manifest = alternative.join("AddIns/example/manifest.xml");
    std::fs::write(&manifest, b"<AddInItem UID=\"example\"/>")?;
    fixture.roots.push(DazipSource {
        root: alternative,
        key: fixture.roots[0].key.clone(),
    });
    fixture.files.push((
        manifest,
        PathBuf::from("alternative.dazip/AddIns/example/manifest.xml"),
    ));
    assert!(fixture.install(None, false, &HashSet::new()).await.is_err());
    assert!(
        fixture
            .tracker
            .list_mods(&fixture.game.id)
            .await?
            .is_empty()
    );
    let excluded = HashSet::from(["alternative.dazip/AddIns/example/manifest.xml".into()]);
    assert!(fixture.install(None, false, &excluded).await.is_ok());
    Ok(())
}
