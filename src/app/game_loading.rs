use std::collections::{HashMap, HashSet};

use relm4::prelude::*;

use crate::core::{
    game,
    tracker::{OverrideInfo, Tracker},
};
use crate::models::{
    game::Game, group::ModGroup, mod_entry::ModEntry, plugin::Plugin, profile::Profile,
};

use super::messages::GamesCmdMsg;
use super::session::{GameLoadMode, load_game_data};
use super::types::LoadedData;
use super::{App, AppCmdMsg};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    sequence: u64,
    game_id: String,
}

#[derive(Default)]
pub(crate) struct State {
    sequence: u64,
    pending: Option<Request>,
    library_ready: bool,
    failed: bool,
}

impl State {
    fn begin(&mut self, game_id: String) -> Request {
        self.clear();
        let request = Request {
            sequence: self.sequence,
            game_id,
        };
        self.pending = Some(request.clone());
        request
    }

    pub(crate) fn clear(&mut self) {
        self.sequence = self.sequence.wrapping_add(1);
        self.pending = None;
        self.library_ready = false;
        self.failed = false;
    }

    fn accepts(&self, request: &Request, selected: Option<&str>) -> bool {
        self.pending.as_ref() == Some(request) && selected == Some(request.game_id.as_str())
    }

    fn finish(&mut self, request: &Request, selected: Option<&str>, failed: bool) -> bool {
        if !self.accepts(request, selected) {
            return false;
        }
        self.pending = None;
        self.failed = failed;
        true
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(crate) fn loading_library(&self) -> bool {
        self.is_pending() && !self.library_ready
    }
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }
}

#[derive(Debug)]
pub(crate) struct Library {
    pub(crate) mods: Vec<ModEntry>,
    pub(crate) plugins: Vec<Plugin>,
    pub(crate) groups: Vec<ModGroup>,
    pub(crate) profiles: Vec<Profile>,
    pub(crate) overrides: HashMap<String, OverrideInfo>,
    pub(crate) override_mod_ids: HashSet<String>,
}

async fn read_library(tracker: &Tracker, game: &Game) -> Result<Library, String> {
    let mods = tracker
        .list_mods(&game.id)
        .await
        .map_err(|error| error.to_string())?;
    let plugins = tracker
        .list_plugins(&game.id)
        .await
        .map_err(|error| error.to_string())?;
    let groups = tracker
        .list_groups(&game.id)
        .await
        .map_err(|error| error.to_string())?;
    let profiles = tracker
        .list_profiles(&game.id)
        .await
        .map_err(|error| error.to_string())?;
    let names = mods
        .iter()
        .map(|entry| (entry.id.clone(), entry.name.clone()))
        .collect();
    let overrides = tracker
        .compute_overrides(&game.id, game::handler_for(&game.engine), &names)
        .await
        .map_err(|error| error.to_string())?;
    let override_mod_ids = if game.engine == crate::models::game::GameEngine::Eclipse {
        tracker
            .eclipse_override_ids(&game.id)
            .await
            .map_err(|error| error.to_string())?
    } else {
        HashSet::new()
    };

    Ok(Library {
        mods,
        plugins,
        groups,
        profiles,
        overrides,
        override_mod_ids,
    })
}

impl App {
    pub(crate) fn begin_game_load(&mut self, sender: &ComponentSender<Self>) {
        let (Some(tracker), Some(game)) =
            (self.session.tracker.clone(), self.selected_game().cloned())
        else {
            return;
        };
        let request = self.session.game_load.begin(game.id.clone());
        self.session.profile_game_id = None;
        self.update_profile_list(Vec::new(), 0);
        self.location_command(sender, async move {
            let result = read_library(&tracker, &game).await;
            AppCmdMsg::Games(GamesCmdMsg::LibraryLoaded { request, result })
        });
    }

    pub(crate) fn library_loaded(
        &mut self,
        request: Request,
        result: Result<Library, String>,
        sender: &ComponentSender<Self>,
    ) {
        if !self
            .session
            .game_load
            .accepts(&request, self.selected_game().map(|game| game.id.as_str()))
        {
            return;
        }
        if let Ok(library) = result {
            self.session.game_load.library_ready = true;
            self.apply_library(library);
        }
        let (Some(tracker), Some(game)) =
            (self.session.tracker.clone(), self.selected_game().cloned())
        else {
            return;
        };
        self.location_command(sender, async move {
            let result = load_game_data(&tracker, &game, GameLoadMode::OpenGame).await;
            AppCmdMsg::Games(GamesCmdMsg::GameOpened { request, result })
        });
    }

    pub(crate) fn game_opened(
        &mut self,
        request: Request,
        result: Result<LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        let selected = self.selected_game().map(|game| game.id.clone());
        if !self
            .session
            .game_load
            .finish(&request, selected.as_deref(), result.is_err())
        {
            return;
        }
        if result.is_err() {
            self.session.location_blocked.insert(request.game_id);
        }
        self.handle_cmd_mods_loaded(result, false, sender);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::game::GameEngine;

    // @variants: both
    #[test]
    fn returning_to_fallout_rejects_both_earlier_game_loads() {
        let mut state = State::default();
        let first = state.begin("fallout-4".into());
        let mele = state.begin("mass-effect-le2".into());
        let returning = state.begin("fallout-4".into());
        assert!(!state.accepts(&first, Some("fallout-4")));
        assert!(!state.accepts(&mele, Some("fallout-4")));
        assert!(state.accepts(&returning, Some("fallout-4")));
        assert!(!state.accepts(&returning, None));
        state.clear();
        assert!(!state.accepts(&returning, Some("fallout-4")));
    }

    // @variants: both
    #[test]
    fn verification_failure_keeps_the_library_and_old_failures_do_not_finish_a_new_load() {
        let mut state = State::default();
        let old = state.begin("fallout-4".into());
        let current = state.begin("fallout-4".into());
        state.library_ready = true;
        assert!(!state.finish(&old, Some("fallout-4"), true));
        assert!(state.is_pending());
        assert!(!state.failed());
        assert!(state.finish(&current, Some("fallout-4"), true));
        assert!(state.library_ready);
        assert!(state.failed());
        assert!(!state.is_pending());
        let retry = state.begin("fallout-4".into());
        assert!(state.loading_library());
        assert!(!state.failed());
        assert!(state.finish(&retry, Some("fallout-4"), false));
        assert!(!state.failed());
    }

    // @variants: both
    #[tokio::test]
    async fn game_switch_reads_saved_mods_without_waiting_for_filesystem_checks()
    -> anyhow::Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let temp = tempfile::tempdir()?;
        let mut games = Vec::new();
        for (id, engine) in [
            ("fallout-4", GameEngine::Bethesda),
            ("mass-effect-le2", GameEngine::MassEffect),
        ] {
            let game = Game {
                id: id.into(),
                title: id.into(),
                engine,
                path: temp.path().join(id),
                data_subdir: "Data".into(),
                wine_prefix: None,
            };
            tracker
                .upsert_game(
                    &game.id,
                    &game.title,
                    &game.path,
                    &game.data_subdir,
                    game.engine.as_str(),
                    None,
                    false,
                )
                .await?;
            tracker.ensure_default_profile(id).await?;
            tracker
                .insert_mod(&ModEntry {
                    id: id.into(),
                    game_id: id.into(),
                    name: id.into(),
                    enabled: true,
                    priority: 4,
                    archive_hash: None,
                    archive_path: None,
                    installed_at: None,
                    nexus_mod_id: None,
                    nexus_file_id: None,
                    nexus_domain: None,
                    version: None,
                    author: None,
                    nexus_description: None,
                    latest_version: None,
                    nexus_file_name: None,
                    nexus_is_primary: false,
                    archive_md5: None,
                    install_target: crate::models::mod_entry::InstallTarget::Data,
                    notes: None,
                })
                .await?;
            games.push(game);
        }
        tracker
            .insert_plugins(&[Plugin {
                id: "fo4-plugin".into(),
                mod_id: "fallout-4".into(),
                filename: "Example.esp".into(),
                load_order: 3,
                enabled: true,
            }])
            .await?;
        for index in [0, 1, 0] {
            let library = read_library(&tracker, &games[index])
                .await
                .map_err(anyhow::Error::msg)?;
            assert_eq!(library.mods.len(), 1);
            assert_eq!(library.mods[0].game_id, games[index].id);
            assert_eq!(library.mods[0].priority, 4);
            assert_eq!(library.plugins.len(), usize::from(index == 0));
            assert!(!games[index].path.exists());
        }
        assert_eq!(tracker.list_plugins("fallout-4").await?[0].load_order, 3);
        assert!(
            tracker
                .load_mele_baseline("mass-effect-le2")
                .await?
                .is_none()
        );
        Ok(())
    }
}
