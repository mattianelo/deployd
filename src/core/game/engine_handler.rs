use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::Result;

use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameEngine};
use crate::models::mod_entry::InstallTarget;

use super::{aurora, bethesda, eclipse, redengine};

pub(crate) trait EngineHandler: Send + Sync {
    /// Rewrite destination paths in the file list per engine rules.
    fn route_file_list(
        &self,
        game: &Game,
        mod_name: &str,
        stripped_wrapper: Option<&str>,
        file_list: Vec<(PathBuf, PathBuf)>,
        file_targets: &HashMap<String, InstallTarget>,
    ) -> Vec<(PathBuf, PathBuf)>;

    /// Returns the conflict detection key for a deployed file path.
    ///
    /// Default: the full lowercase path, so conflict detection is path-exact.
    /// Aurora overrides this to return just the filename for Override/ files,
    /// matching how the Aurora engine resolves collisions at load time:
    /// `override/ModA/path/foo.xml` and `override/foo.xml` both surface as
    /// `foo.xml` and therefore conflict regardless of subfolder depth.
    fn conflict_key<'a>(&self, game_rel_lowercase: &'a str) -> &'a str {
        game_rel_lowercase
    }

    /// Whether this conflict key should be excluded from conflict reporting.
    ///
    /// Filters out common non-meaningful files (readme, license, changelog, etc.)
    /// that are present in many mods and would produce noise in conflict indicators.
    fn is_conflict_key_ignored(&self, conflict_key: &str) -> bool {
        let basename = conflict_key.rsplit('/').next().unwrap_or(conflict_key);
        crate::core::installer::is_ignorable_file(basename)
    }

    /// Whether `file_key` should deploy to the game root (vs. the data subdir).
    ///
    /// Default: explicit-root files honour `file_targets` with a Root fallback;
    /// all others honour `file_targets` with a Data fallback.
    fn deploy_to_root(
        &self,
        file_key: &str,
        file_targets: &HashMap<String, InstallTarget>,
        explicit_root: bool,
    ) -> bool {
        if explicit_root {
            file_targets
                .get(file_key)
                .cloned()
                .unwrap_or(InstallTarget::Root)
                == InstallTarget::Root
        } else {
            file_targets
                .get(file_key)
                .cloned()
                .unwrap_or(InstallTarget::Data)
                == InstallTarget::Root
        }
    }

    /// Absolute path to the directory where mod files are deployed.
    ///
    /// Default: `game.data_dir()`. Eclipse overrides this to the Wine user dir.
    fn deploy_dir(&self, game: &Game) -> PathBuf {
        game.data_dir()
    }

    /// Directory to search for tool executables.
    ///
    /// Default: the deploy dir itself. Eclipse overrides to go two levels up
    /// (the Wine Documents folder).
    fn tool_search_dir(&self, game: &Game) -> Option<PathBuf> {
        Some(self.deploy_dir(game))
    }

    /// Engine-specific steps run after all files are hard-linked.
    ///
    /// Returns a boxed future for trait-object compatibility. Default: no-op.
    fn post_deploy<'a>(
        &'a self,
        _game: &'a Game,
        _tracker: &'a Tracker,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

/// Return the `EngineHandler` for the given engine.
///
/// Each handler is a zero-sized unit struct stored as a crate-level static,
/// so this is a cheap pointer return with no heap allocation.
pub(crate) fn handler_for(engine: &GameEngine) -> &'static dyn EngineHandler {
    static BETHESDA: bethesda::BethesdaHandler = bethesda::BethesdaHandler;
    static REDENGINE: redengine::REDEngineHandler = redengine::REDEngineHandler;
    static ECLIPSE: eclipse::EclipseHandler = eclipse::EclipseHandler;
    static AURORA: aurora::AuroraHandler = aurora::AuroraHandler;

    match engine {
        GameEngine::Bethesda => &BETHESDA,
        GameEngine::REDEngine => &REDENGINE,
        GameEngine::Eclipse => &ECLIPSE,
        GameEngine::Aurora => &AURORA,
    }
}
