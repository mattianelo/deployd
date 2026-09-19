use super::*;
use crate::core::game::mass_effect::helper::Job;

// @variants: both
#[tokio::test]
async fn le2_plot_deployment_uses_binary_mounts_and_restores_originals() -> Result<()> {
    let fixture = Fixture::with_game(Target::Le2, |game| {
        for name in ["WwiseAudio.pcc", "Startup_INT.pcc"] {
            fs::write(
                game.join("BioGame/CookedPCConsole").join(name),
                b"dependency",
            )?;
        }
        Ok(())
    })
    .await?;
    let package = fixture
        .merge_package("DLC_MOD_Plot", 5, &["plot", "extra_plot"])
        .await?;
    let recipe = fixture.merge_recipe(vec![package]);
    let backend = fixture.backend("assert len(request['contributions']) == 2\nassert request['game'] == 'LE2'\nassert 'CookedPCConsole/WwiseAudio.pcc' in [item['path'] for item in request['dependencies']]\nassert 'CookedPCConsole/Startup_INT.pcc' in [item['path'] for item in request['dependencies']]\nassert 'CookedPCConsole/SFXStrategicAI.pcc' not in [item['path'] for item in request['dependencies']]")?;
    let first = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    let path = fixture
        .game
        .path
        .join("BioGame/CookedPCConsole/PlotManager.pcc");
    assert_eq!(fs::read(&path)?, b"dependency-merged");
    let rebuilt = fixture
        .deploy_merges(recipe, Some(&first), backend, cancel(), Arc::new(|_, _| {}))
        .await?;
    assert_eq!(fs::read(&path)?, b"dependency-merged");
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&rebuilt))
        .await?;
    assert_eq!(fs::read(&path)?, b"dependency");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn queued_folder_recovery_waits_through_merges_and_journal_publication() -> Result<()> {
    for abandon in [false, true] {
        let fixture = Fixture::new(Target::Le1).await?;
        let package = fixture.merge_package("DLC_MOD_Test", 5, &["table"]).await?;
        let backend = fixture.backend(
            "while not (pathlib.Path(sys.argv[1]).parent / 'release').exists(): time.sleep(0.01)",
        )?;
        let release = backend
            .assembly
            .parent()
            .context("missing backend")?
            .join("release");
        let plan = fixture.inspect(fixture.merge_recipe(vec![package])).await?;
        let (arrived, waiting) = tokio::sync::oneshot::channel();
        let arrived = std::sync::Mutex::new(Some(arrived));
        let task = tokio::spawn(deploy_in(
            fixture.tracker.clone(),
            Destination {
                repair_components: false,
                game: fixture.game.clone(),
                profile: fixture.profile.clone(),
                previous: None,
                backend: Some(backend),
            },
            plan,
            fixture.data.clone(),
            cancel(),
            Arc::new(move |_, _| {
                if let Some(sender) = arrived.lock().unwrap().take() {
                    let _ = sender.send(());
                }
            }),
        ));
        tokio::time::timeout(std::time::Duration::from_secs(10), waiting).await??;
        let writer = tokio::spawn(crate::core::location_recovery::activity_lock().write_owned());
        tokio::task::yield_now().await;
        if abandon {
            task.abort();
            assert!(task.await.is_err());
        } else {
            fs::write(release, b"release")?;
            tokio::time::timeout(std::time::Duration::from_secs(10), task).await???;
        }
        let _writer = tokio::time::timeout(std::time::Duration::from_secs(10), writer).await??;
        assert_eq!(
            fs::read(fixture.game.path.join(ENGINE))?,
            if abandon {
                b"original".as_slice()
            } else {
                b"original-merged".as_slice()
            }
        );
        assert_eq!(fs::read_dir(fixture.data.join("mele-rebuilds"))?.count(), 0);
        assert_eq!(
            fs::read_dir(fixture.data.join("mele-transformations"))?.count(),
            0
        );
    }
    Ok(())
}

impl Fixture {
    pub(super) async fn merge_package(
        &self,
        dlc: &str,
        mount: i32,
        kinds: &[&str],
    ) -> Result<Package> {
        let source = self.temp.path().join(Uuid::new_v4().to_string());
        let cooked = source.join(dlc).join("CookedPCConsole");
        fs::create_dir_all(&cooked)?;
        let game = self.recipe(Vec::new()).target;
        let game_name = serde_json::to_value(game)?
            .as_str()
            .context("missing game name")?
            .to_owned();
        if game == Target::Le1 {
            fs::write(
                source.join(dlc).join("AutoLoad.ini"),
                format!("[ME1DLCMOUNT]\nModMount={mount}"),
            )?;
        } else {
            let mut data = vec![0_u8; 44];
            for (index, value) in [684_u32, 168, 65643].into_iter().enumerate() {
                data[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            data[12..16].copy_from_slice(&mount.to_le_bytes());
            fs::write(cooked.join("Mount.dlc"), data)?;
        }
        let version = if kinds.contains(&"extra_plot") {
            "9.2"
        } else {
            "9.1"
        };
        fs::write(
            source.join("moddesc.ini"),
            format!(
                "[ModManager]\ncmmver={version}\n[ModInfo]\ngame={game_name}\nmodname=Merge example\nmodver=1.0\nmoddev=Author\nmoddesc=Example\n[CUSTOMDLC]\nsourcedirs={dlc}\ndestdirs={dlc}\n"
            ),
        )?;
        for kind in kinds {
            match *kind {
                "table" => {
                    fs::write(cooked.join("Table.pcc"), b"table")?;
                    fs::write(
                        cooked.join(format!("{dlc}-table.m3da")),
                        r#"[{"packagefile":"Engine.pcc","mergepackagefile":"Table.pcc","mergetables":["Example_part"]}]"#,
                    )?;
                }
                "extra_plot" => fs::write(
                    cooked.join("Additional.pmu"),
                    "public function bool F2(BioWorldInfo bi, int n)\n{\n return false;\n}\n",
                )?,
                "plot" => fs::write(
                    cooked.join("PlotManagerUpdate.pmu"),
                    "public function bool F1(BioWorldInfo bi, int n)\n{\n return true;\n}\n",
                )?,
                _ => fs::write(
                    cooked.join(format!("ConfigDelta-{kind}.m3cd")),
                    "[BioUI.ini Engine.UI]\n+Key=Value",
                )?,
            }
        }
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

    pub(super) fn backend(&self, suffix: &str) -> Result<helper::Backend> {
        let parent = self.temp.path().join("backend");
        fs::create_dir_all(&parent)?;
        let backend = helper::Backend {
            runtime: parent.join("runtime"),
            assembly: parent.join("Deployd.Mele.dll"),
            native_library: parent.join("libdeployd_oodle.so"),
        };
        let capabilities = serde_json::json!({"protocol":1,"backend":"deployd-mele","version":helper::protocol::VERSION,
            "games":["LE1","LE2","LE3"],"capabilities":["le1-m3da","le1-m3cd","mele-plot","mele-m3m-ordered","le1-tlk","mele-merge-dlc","mele-m3gs"],"validation":["package-roundtrip"]});
        fs::write(
            &backend.runtime,
            format!(
                r#"#!/usr/bin/python3
import hashlib, json, pathlib, sys, time
if sys.argv[2] == 'capabilities':
 print({capabilities:?})
 sys.exit(0)
request = json.loads(pathlib.Path(sys.argv[3]).read_text())
outputs = []
if request['operation'] == 'mele-merge-dlc':
 for path in request['outputs']:
  value = b'generated-merge'
  output = pathlib.Path(request['output_root']) / path
  output.parent.mkdir(parents=True, exist_ok=True)
  output.write_bytes(value)
  outputs.append(dict(path=path, size=len(value), sha256=hashlib.sha256(value).hexdigest()))
 targets = []
else:
 targets = request.get('targets', [request.get('target')])
for pair in targets:
 path = pair['path'] if request['operation'] == 'le1-tlk' else pair['current']['path']
 if request['operation'] in ['mele-m3m-ordered', 'le1-tlk']:
  original = (pathlib.Path(request['input_root']) / path).read_bytes()
  value = original + (b'-m3m' if request['operation'] == 'mele-m3m-ordered' else b'-tlk')
 else:
  original = (pathlib.Path(request['original_root']) / path).read_bytes()
  value = original + b'-merged'
 output = pathlib.Path(request['output_root']) / path
 output.parent.mkdir(parents=True, exist_ok=True)
 output.write_bytes(value)
 outputs.append(dict(path=path, size=len(value), sha256=hashlib.sha256(value).hexdigest()))
total = 7 if request['operation'] == 'mele-merge-dlc' else len(request.get('contributions', request.get('jobs', request.get('changes', []))))
for done in range(1, total + 1):
 print(json.dumps(dict(protocol=1, type='progress', completed=done, total=total)), flush=True)
print(json.dumps(dict(protocol=1, type='complete', outputs=outputs)), flush=True)
{suffix}
"#,
                capabilities = capabilities.to_string()
            ),
        )?;
        fs::set_permissions(&backend.runtime, fs::Permissions::from_mode(0o700))?;
        fs::write(&backend.assembly, b"fixture")?;
        fs::write(&backend.native_library, b"fixture")?;
        Ok(backend)
    }

    pub(super) async fn deploy_merges(
        &self,
        recipe: Recipe,
        previous: Option<&journal::State>,
        backend: helper::Backend,
        cancelled: Arc<AtomicBool>,
        progress: Progress,
    ) -> Result<journal::State> {
        let plan = self.inspect(recipe).await?;
        deploy_in(
            self.tracker.clone(),
            Destination {
                repair_components: false,
                game: self.game.clone(),
                profile: self.profile.clone(),
                previous: previous.map(|state| state.generation.clone()),
                backend: Some(backend),
            },
            plan,
            self.data.clone(),
            cancelled,
            progress,
        )
        .await
    }

    pub(super) fn merge_recipe(&self, packages: Vec<Package>) -> Recipe {
        let mut recipe = self.recipe(packages);
        recipe.helper_version = Some(helper::protocol::VERSION.into());
        recipe
    }
}

// @variants: both
#[tokio::test]
async fn plans_mount_order_and_generated_outputs_separately_from_file_priority() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let high = fixture
        .merge_package("DLC_MOD_High", 20, &["table", "second", "first", "plot"])
        .await?;
    let low = fixture
        .merge_package("DLC_MOD_Low", 5, &["table", "first"])
        .await?;
    let recipe = fixture.merge_recipe(vec![high, low]);
    let plan = fixture.inspect(recipe).await?;
    assert_eq!(plan.generated_files().len(), 4);
    assert!(!plan.generated_files().contains("CookedPCConsole/Core.pcc"));
    let current = candidate_inputs(&plan)?;
    let Some(Job::Tables { contributions, .. }) = plan.merges.job(0, &current)? else {
        panic!("missing tables")
    };
    assert_eq!(
        contributions
            .iter()
            .map(|item| item.mount)
            .collect::<Vec<_>>(),
        [5, 20]
    );
    let Some(Job::Config {
        contributions,
        targets,
    }) = plan.merges.job(1, &current)?
    else {
        panic!("missing configuration")
    };
    assert_eq!(targets.len(), 2);
    assert_eq!(
        contributions
            .iter()
            .map(|item| item.mount)
            .collect::<Vec<_>>(),
        [5, 20, 20]
    );
    assert!(
        contributions[1]
            .manifest
            .path
            .ends_with("ConfigDelta-first.m3cd")
    );
    let mut current = current;
    current
        .get_mut("CookedPCConsole/Engine.pcc")
        .context("missing engine")?
        .sha256 = "a".repeat(64);
    let Some(Job::Plot { dependencies, .. }) = plan.merges.job(2, &current)? else {
        panic!("missing plot")
    };
    assert_eq!(
        dependencies
            .iter()
            .find(|file| file.path.ends_with("/Engine.pcc"))
            .context("missing dependency")?
            .sha256,
        "a".repeat(64)
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn plans_merges_from_winning_manifests_and_effective_autoload_files() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let first = fixture
        .merge_package("DLC_MOD_Test", 5, &["table", "first"])
        .await?;
    let last = fixture
        .merge_package("DLC_MOD_Test", 20, &["table", "second"])
        .await?;
    let mut recipe = fixture.merge_recipe(vec![first, last]);
    let plan = fixture.inspect(recipe.clone()).await?;
    let Some(Job::Tables { contributions, .. }) = plan.merges.job(0, &candidate_inputs(&plan)?)?
    else {
        panic!("missing tables")
    };
    assert_eq!(contributions.len(), 1);
    assert_eq!(contributions[0].mount, 20);
    let Some(Job::Config { contributions, .. }) = plan.merges.job(1, &candidate_inputs(&plan)?)?
    else {
        panic!("missing configuration")
    };
    assert_eq!(contributions.len(), 2);
    assert!(contributions.iter().all(|item| item.mount == 20));
    recipe.packages[1].enabled = false;
    let plan = fixture.inspect(recipe).await?;
    let Some(Job::Config { contributions, .. }) = plan.merges.job(1, &candidate_inputs(&plan)?)?
    else {
        panic!("missing configuration")
    };
    assert_eq!(contributions.len(), 1);
    assert_eq!(contributions[0].mount, 5);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn external_dependency_edits_before_or_during_merges_block_publication() -> Result<()> {
    for during in [false, true] {
        let fixture = Fixture::new(Target::Le1).await?;
        let package = fixture.merge_package("DLC_MOD_Test", 5, &["plot"]).await?;
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
        let dependency = fixture.game.path.join("BioGame/CookedPCConsole/Core.pcc");
        if !during {
            fs::write(&dependency, b"external edit")?;
        }
        let path = dependency.clone();
        let progress: Progress = Arc::new(move |_, _| {
            if during {
                fs::write(&path, b"external edit").unwrap();
            }
        });
        assert!(
            fixture
                .deploy_merges(recipe, Some(&installed), backend, cancel(), progress)
                .await
                .is_err()
        );
        assert_eq!(fs::read(dependency)?, b"external edit");
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
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rebuilds_target_merges_and_restores_originals_when_disabled() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let package = fixture
        .merge_package("DLC_MOD_Test", 5, &["table", "config", "plot"])
        .await?;
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
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original-merged"
    );
    assert!(
        !installed
            .files
            .iter()
            .any(|file| file.relative.ends_with("/Core.pcc"))
    );
    assert_eq!(installed.recipe, Some(recipe.clone()));
    let reopened = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture.temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    assert_eq!(
        reopened.mele_deployment(&fixture.game.id).await?,
        Some(installed.clone())
    );
    let rebuilt = fixture
        .deploy_merges(
            recipe.clone(),
            Some(&installed),
            backend,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(rebuilt.files, installed.files);
    let mut disabled = recipe;
    disabled.packages[0].enabled = false;
    let removed = fixture.deploy(disabled, Some(&rebuilt)).await?;
    assert!(removed.files.is_empty());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert_eq!(
        fs::read(
            fixture
                .game
                .path
                .join("BioGame/CookedPCConsole/PlotManager.pcc")
        )?,
        b"dependency"
    );
    assert!(
        !fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_Test")
            .join("AutoLoad.ini")
            .exists()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failed_or_cancelled_merges_preserve_the_previous_deployment() -> Result<()> {
    for failure in [
        "output.write_bytes(b'bad')",
        "sys.exit(1)",
        "cancel",
        "database",
    ] {
        let fixture = Fixture::new(Target::Le1).await?;
        let package = fixture.merge_package("DLC_MOD_Test", 5, &["table"]).await?;
        let recipe = fixture.merge_recipe(vec![package]);
        let installed = fixture
            .deploy_merges(
                recipe.clone(),
                None,
                fixture.backend("")?,
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        let cancelled = cancel();
        let stop = cancelled.clone();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = events.clone();
        let progress: Progress = Arc::new(move |done, total| {
            recorded.lock().unwrap().push((done, total));
            if failure == "cancel" {
                stop.store(true, Ordering::Release);
            }
        });
        let backend = fixture.backend(if ["cancel", "database"].contains(&failure) {
            ""
        } else {
            failure
        })?;
        if failure == "database" {
            sqlx::query("CREATE TRIGGER reject_recipe BEFORE UPDATE ON mele_recipes BEGIN SELECT RAISE(ABORT, 'recipe failure'); END")
                .execute(&fixture.tracker.pool).await?;
            let script = fs::read_to_string(&backend.runtime)?.replace("b'-merged'", "b'-updated'");
            fs::write(&backend.runtime, script)?;
        }
        assert!(
            fixture
                .deploy_merges(recipe, Some(&installed), backend, cancelled, progress)
                .await
                .is_err()
        );
        {
            let events = events.lock().unwrap();
            assert!(events.iter().all(|(done, total)| done < total));
            assert!(
                events
                    .windows(2)
                    .all(|pair| pair[0].0 <= pair[1].0 && pair[0].1 == pair[1].1)
            );
        }
        assert_eq!(
            fixture.tracker.mele_deployment(&fixture.game.id).await?,
            Some(installed)
        );
        assert_eq!(
            fs::read(fixture.game.path.join(ENGINE))?,
            b"original-merged"
        );
        assert!(
            fixture
                .tracker
                .mele_journal(&fixture.game.id)
                .await?
                .is_none()
        );
        assert_eq!(fs::read_dir(fixture.data.join("mele-rebuilds"))?.count(), 0);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_conflicting_mounts_missing_originals_and_future_helper_versions() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let table = fixture.merge_package("DLC_MOD_A", 5, &["table"]).await?;
    let plot = fixture.merge_package("DLC_MOD_B", 5, &["plot"]).await?;
    assert!(
        fixture
            .inspect(fixture.merge_recipe(vec![table.clone(), plot]))
            .await
            .is_err()
    );
    let mut recipe = fixture.merge_recipe(vec![table.clone()]);
    recipe.helper_version = Some("99.0.0".into());
    assert!(recipe.validate().is_err());
    for version in [
        "0.7.1", "0.8.0", "0.9.0", "0.9.1", "0.9.2", "0.10.0", "0.11.0",
    ] {
        recipe.helper_version = Some(version.into());
        recipe.validate()?;
        assert!(fixture.inspect(recipe.clone()).await.is_err());
    }
    let mut legacy = serde_json::to_value(fixture.recipe(Vec::new()))?;
    legacy
        .as_object_mut()
        .context("missing recipe object")?
        .remove("helper_version");
    assert_eq!(
        serde_json::from_value::<Recipe>(legacy)?.helper_version,
        None
    );
    let mut baseline = fixture.baseline.clone();
    baseline.files.retain(|file| file.relative != ENGINE);
    let plan = fixture.inspect(fixture.merge_recipe(vec![table])).await?;
    assert!(
        super::super::merges::Merges::inspect(
            &plan.files,
            &plan.packages,
            &baseline,
            &BTreeSet::new(),
            &Control::recovery()
        )
        .is_err()
    );
    Ok(())
}

fn candidate_inputs(plan: &ValidatedRecipe) -> Result<BTreeMap<String, helper::FileIdentity>> {
    let mut current = plan.merges.originals.clone();
    for file in &plan.files {
        let input = super::super::merges::identity(&file.destination)?;
        current.insert(input.path.clone(), input);
    }
    Ok(current)
}
