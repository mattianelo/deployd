use std::os::unix::fs::symlink;

use super::*;

use crate::models::game::GameEngine;

// @variants: both
#[test]
fn inspects_each_engine_anchor_without_reinterpreting_other_engines() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let prefix = temp.path().join("prefix");
    fs::create_dir_all(prefix.join("drive_c/users/player"))?;
    let mut game = Game {
        id: "skyrim-se".into(),
        title: "Game".into(),
        path: temp.path().join("game"),
        data_subdir: "Data".into(),
        engine: GameEngine::Bethesda,
        wine_prefix: Some(prefix),
    };
    fs::create_dir_all(&game.path)?;
    let cases = [
        (
            GameEngine::Bethesda,
            Target::Bethesda {
                root: false,
                path: "mod.txt".into(),
            },
        ),
        (
            GameEngine::Bethesda,
            Target::Bethesda {
                root: true,
                path: "root.txt".into(),
            },
        ),
        (GameEngine::Bethesda, Target::PluginControl { slot: 0 }),
        (GameEngine::Bethesda, Target::CustomIni { slot: 0 }),
        (
            GameEngine::Aurora,
            Target::Aurora {
                root: false,
                path: "mod.txt".into(),
            },
        ),
        (
            GameEngine::Aurora,
            Target::Aurora {
                root: true,
                path: "system/mod.txt".into(),
            },
        ),
        (
            GameEngine::Aurora,
            Target::Aurora {
                root: true,
                path: "launcher/mod.txt".into(),
            },
        ),
        (
            GameEngine::Aurora,
            Target::Aurora {
                root: true,
                path: "register/mod.txt".into(),
            },
        ),
        (
            GameEngine::Eclipse,
            Target::Eclipse {
                documents: false,
                path: "mod.txt".into(),
            },
        ),
        (
            GameEngine::Eclipse,
            Target::Eclipse {
                documents: true,
                path: "Settings/mod.txt".into(),
            },
        ),
        (
            GameEngine::REDEngine,
            Target::Redengine {
                root: false,
                path: "mod.txt".into(),
            },
        ),
        (
            GameEngine::REDEngine,
            Target::Redengine {
                root: true,
                path: "root.txt".into(),
            },
        ),
        (
            GameEngine::MassEffect,
            Target::MassEffect {
                path: "BioGame/mod.txt".into(),
            },
        ),
    ];
    for (engine, target) in cases {
        game.engine = engine.clone();
        game.id = if engine == GameEngine::Eclipse {
            "dragon-age"
        } else {
            "skyrim-se"
        }
        .into();
        game.data_subdir = if engine == GameEngine::Eclipse {
            "Documents/BioWare/Dragon Age/packages/core/override"
        } else {
            "Data"
        }
        .into();
        let path = target.resolve(&game)?;
        fs::create_dir_all(path.parent().context("Target parent")?)?;
        fs::write(&path, b"managed")?;
        assert!(matches!(
            live(&game, &target, &Control::default())?,
            Node::File { .. }
        ));
        for other in [
            GameEngine::Bethesda,
            GameEngine::Aurora,
            GameEngine::Eclipse,
            GameEngine::REDEngine,
            GameEngine::MassEffect,
        ] {
            if other == engine {
                continue;
            }
            game.engine = other;
            assert!(live(&game, &target, &Control::default()).is_err());
        }
        game.engine = engine;
    }
    Ok(())
}

// @variants: both
#[test]
fn missing_parents_are_missing_files_but_redirected_parents_block_inspection() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let game = Game {
        id: "game".into(),
        title: "Game".into(),
        path: temp.path().to_owned(),
        data_subdir: "Data".into(),
        engine: GameEngine::Bethesda,
        wine_prefix: None,
    };
    let target = Target::Bethesda {
        root: false,
        path: "nested/file.txt".into(),
    };
    assert_eq!(live(&game, &target, &Control::default())?, Node::Absent);
    let outside = tempfile::tempdir()?;
    fs::create_dir(outside.path().join("nested"))?;
    fs::write(outside.path().join("nested/file.txt"), b"outside")?;
    symlink(outside.path(), game.path.join("Data"))?;
    assert!(live(&game, &target, &Control::default()).is_err());
    assert_eq!(
        fs::read(outside.path().join("nested/file.txt"))?,
        b"outside"
    );
    Ok(())
}
