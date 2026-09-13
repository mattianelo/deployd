use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{KnownGameOption, known_game_options, mass_effect::Target};
use crate::models::game::{Game, GameConfig, GameEngine};
use crate::utils::location::{FolderRole, FolderSelection};

pub(crate) const MELE_FAMILY_ID: &str = "mass-effect-legendary-edition";

pub(crate) fn options() -> Vec<KnownGameOption> {
    let mut options: Vec<_> = known_game_options()
        .into_iter()
        .filter(|option| *option.engine != GameEngine::MassEffect)
        .collect();
    options.push(KnownGameOption {
        deployd_id: MELE_FAMILY_ID,
        title: "Mass Effect Legendary Edition (all three games)",
        data_subdir: "BioGame",
        engine: &GameEngine::MassEffect,
    });
    options
}

pub(crate) fn expand(configs: Vec<GameConfig>) -> Result<Vec<GameConfig>> {
    let mut expanded = Vec::new();
    for config in configs {
        if config.game.id == MELE_FAMILY_ID {
            expanded.extend(expand_family(config)?);
        } else {
            expanded.push(config);
        }
    }
    let mut ids = HashSet::new();
    for config in &expanded {
        if !ids.insert(&config.game.id) {
            bail!("{} is already included in setup", config.game.title);
        }
    }
    Ok(expanded)
}

fn expand_family(config: GameConfig) -> Result<Vec<GameConfig>> {
    let selection = config
        .locations
        .iter()
        .find(|location| location.role == FolderRole::Game)
        .context(
            "Select the Legendary Edition installation folder before adding the three games",
        )?;
    if selection.location.root != config.game.path || !selection.relative.as_os_str().is_empty() {
        bail!("Select the Legendary Edition folder containing Game/ME1, Game/ME2 and Game/ME3");
    }
    let canonical_root = fs::canonicalize(&config.game.path).context(
        "Cannot access the selected Legendary Edition folder; restore folder access and try again",
    )?;
    let mut configs = Vec::new();
    for (target, relative, executable) in [
        (Target::Le1, "Game/ME1", "MassEffect1.exe"),
        (Target::Le2, "Game/ME2", "MassEffect2.exe"),
        (Target::Le3, "Game/ME3", "MassEffect3.exe"),
    ] {
        let path = config.game.path.join(relative);
        validate_member(&canonical_root, &path.join("BioGame"), false)?;
        validate_member(
            &canonical_root,
            &path.join("Binaries/Win64").join(executable),
            true,
        )?;
        let option = known_game_options()
            .into_iter()
            .find(|option| option.deployd_id == target.game_id())
            .context("Legendary Edition game registration is missing")?;
        let mut locations = config.locations.clone();
        locations.retain(|location| location.role != FolderRole::Game);
        locations.push(FolderSelection {
            role: FolderRole::Game,
            location: selection.location.clone(),
            relative: relative.into(),
        });
        configs.push(GameConfig {
            game: Game {
                id: target.game_id().to_string(),
                title: option.title.to_string(),
                path,
                data_subdir: "BioGame".to_string(),
                engine: GameEngine::MassEffect,
                wine_prefix: config.game.wine_prefix.clone(),
            },
            custom: config.custom,
            locations,
        });
    }
    Ok(configs)
}

fn validate_member(root: &Path, path: &Path, file: bool) -> Result<()> {
    let canonical = fs::canonicalize(path).with_context(|| {
        format!(
            "Legendary Edition setup requires all three games; cannot access '{}'",
            path.display()
        )
    })?;
    if !canonical.starts_with(root) {
        bail!(
            "Legendary Edition content escapes the selected installation folder: '{}'",
            path.display()
        );
    }
    let metadata = fs::metadata(&canonical)?;
    if (file && !metadata.is_file()) || (!file && !metadata.is_dir()) {
        bail!(
            "Unexpected Legendary Edition installation entry: '{}'",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::location::SelectedLocation;

    fn family(root: &Path) -> Result<GameConfig> {
        for number in 1..=3 {
            fs::create_dir_all(root.join(format!("Game/ME{number}/BioGame")))?;
            let binaries = root.join(format!("Game/ME{number}/Binaries/Win64"));
            fs::create_dir_all(&binaries)?;
            fs::write(binaries.join(format!("MassEffect{number}.exe")), b"fixture")?;
        }
        Ok(GameConfig {
            game: Game {
                id: MELE_FAMILY_ID.into(),
                title: "MELE".into(),
                path: root.into(),
                data_subdir: "BioGame".into(),
                engine: GameEngine::MassEffect,
                wine_prefix: Some("/separately/granted/prefix".into()),
            },
            custom: true,
            locations: vec![
                FolderSelection {
                    role: FolderRole::Game,
                    location: SelectedLocation {
                        root: root.into(),
                        host_hint: None,
                    },
                    relative: Default::default(),
                },
                FolderSelection {
                    role: FolderRole::Prefix,
                    location: SelectedLocation {
                        root: "/separately/granted/prefix".into(),
                        host_hint: None,
                    },
                    relative: Default::default(),
                },
            ],
        })
    }

    // @variants: both
    #[test]
    fn groups_three_games_without_inferring_prefix_authorization() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let configs = expand(vec![family(temp.path())?])?;
        assert_eq!(configs.len(), 3);
        for (index, config) in configs.iter().enumerate() {
            assert_eq!(config.game.id, format!("mass-effect-le{}", index + 1));
            let binding = config
                .locations
                .iter()
                .find(|location| location.role == FolderRole::Game)
                .unwrap();
            assert_eq!(binding.location.root, temp.path());
            assert_eq!(
                binding.relative,
                Path::new(&format!("Game/ME{}", index + 1))
            );
            assert_eq!(
                config
                    .locations
                    .iter()
                    .find(|location| location.role == FolderRole::Prefix)
                    .unwrap()
                    .location
                    .root,
                Path::new("/separately/granted/prefix")
            );
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_incomplete_families_and_duplicate_selections() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config = family(temp.path())?;
        assert!(expand(vec![config.clone(), config.clone()]).is_err());
        fs::remove_file(temp.path().join("Game/ME3/Binaries/Win64/MassEffect3.exe"))?;
        assert!(expand(vec![config]).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_game_content_outside_the_selected_root() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let config = family(temp.path())?;
        let bio = temp.path().join("Game/ME1/BioGame");
        fs::remove_dir(&bio)?;
        std::os::unix::fs::symlink(outside.path(), bio)?;
        assert!(expand(vec![config]).is_err());
        Ok(())
    }

    #[test]
    fn shows_one_family_option_and_preserves_other_games() {
        let options = options();
        assert_eq!(
            options
                .iter()
                .filter(|option| *option.engine == GameEngine::MassEffect)
                .count(),
            1
        );
        for registered in known_game_options()
            .iter()
            .filter(|option| *option.engine != GameEngine::MassEffect)
        {
            assert!(
                options
                    .iter()
                    .any(|option| option.deployd_id == registered.deployd_id)
            );
        }
    }
}
