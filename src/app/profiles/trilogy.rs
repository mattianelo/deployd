use adw::prelude::*;
use relm4::prelude::*;

use crate::core::tracker::trilogy_profiles::{Candidate, Mapping};
use crate::models::profile::SaveMode;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg, GamesCmdMsg, GamesMsg};

impl App {
    pub(crate) fn load_trilogy_candidates(&mut self, sender: &ComponentSender<Self>) {
        self.ui.profile_menu_btn.popdown();
        let (Some(tracker), Some(game)) = (self.session.tracker.clone(), self.selected_game())
        else {
            return;
        };
        let game = game.id.clone();
        self.location_command(sender, async move {
            let result = tracker
                .trilogy_candidates(&game)
                .await
                .map_err(|e| format!("{e:#}"));
            AppCmdMsg::Games(GamesCmdMsg::TrilogyCandidates(game, result))
        });
    }

    pub(crate) fn show_trilogy_mapping(
        &mut self,
        game: String,
        result: Result<Vec<Candidate>, String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if self
            .selected_game()
            .is_none_or(|selected| selected.id != game)
        {
            return;
        }
        let candidates = match result {
            Ok(candidates) => candidates,
            Err(error) => {
                self.push_notification(&error);
                return;
            }
        };
        if candidates.len() != 3
            || candidates
                .iter()
                .any(|candidate| candidate.profiles.is_empty())
        {
            self.show_toast("Grouping needs an ungrouped profile for each game. New profiles already cover the trilogy.");
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Group trilogy profiles")
            .body("Choose the existing profile for each game. Their mod lists, save banks and backups stay intact. Unmatched profiles remain available. The shared save mode takes effect separately on each game's successful Deploy.")
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let name = adw::EntryRow::builder()
            .title("Trilogy profile name")
            .build();
        let preferences = adw::PreferencesGroup::new();
        preferences.add(&name);
        let mut selectors = Vec::new();
        for candidate in &candidates {
            let labels: Vec<String> = candidate
                .profiles
                .iter()
                .map(|profile| {
                    format!(
                        "{} — {}",
                        profile.name,
                        if profile.save_mode == SaveMode::Global {
                            "Global saves"
                        } else {
                            "Profile saves"
                        }
                    )
                })
                .collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let selector = adw::ComboRow::builder()
                .title(match candidate.game_id.as_str() {
                    "mass-effect-le1" => "LE1",
                    "mass-effect-le2" => "LE2",
                    _ => "LE3",
                })
                .model(&gtk::StringList::new(&labels))
                .selected(gtk::INVALID_LIST_POSITION)
                .build();
            preferences.add(&selector);
            selectors.push(selector);
        }
        let mode = adw::ComboRow::builder()
            .title("Trilogy save bank")
            .model(&gtk::StringList::new(&["Global", "Profile-specific"]))
            .selected(gtk::INVALID_LIST_POSITION)
            .build();
        preferences.add(&mode);
        content.append(&preferences);
        dialog.set_extra_child(Some(&content));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("group", "Group profiles");
        dialog.set_response_appearance("group", adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled("group", false);
        dialog.set_close_response("cancel");
        let update = {
            let dialog = dialog.downgrade();
            let name = name.downgrade();
            let selectors: Vec<_> = selectors
                .iter()
                .map(|selector| selector.downgrade())
                .collect();
            let mode = mode.downgrade();
            std::rc::Rc::new(move || {
                if let (Some(dialog), Some(name), Some(mode)) =
                    (dialog.upgrade(), name.upgrade(), mode.upgrade())
                {
                    dialog.set_response_enabled(
                        "group",
                        !name.text().trim().is_empty()
                            && mode.selected() != gtk::INVALID_LIST_POSITION
                            && selectors.iter().all(|selector| {
                                selector.upgrade().is_some_and(|selector| {
                                    selector.selected() != gtk::INVALID_LIST_POSITION
                                })
                            }),
                    );
                }
            })
        };
        let refresh = update.clone();
        name.connect_changed(move |_| refresh());
        for selector in selectors.iter().chain(std::iter::once(&mode)) {
            let refresh = update.clone();
            selector.connect_selected_notify(move |_| refresh());
        }
        let input = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response != "group" {
                return;
            }
            let ids: Vec<String> = candidates
                .iter()
                .zip(&selectors)
                .filter_map(|(candidate, selector)| {
                    candidate
                        .profiles
                        .get(selector.selected() as usize)
                        .map(|profile| profile.id.clone())
                })
                .collect();
            let Ok(profiles) = ids.try_into() else {
                return;
            };
            let mapping = Mapping {
                name: name.text().to_string(),
                profiles,
                mode: if mode.selected() == 0 {
                    SaveMode::Global
                } else {
                    SaveMode::ProfileSpecific
                },
            };
            let _ = input.send(AppMsg::Games(GamesMsg::ApplyTrilogyProfiles(
                game.clone(),
                mapping,
            )));
        });
        dialog.present(Some(root));
    }

    pub(crate) fn apply_trilogy_mapping(
        &mut self,
        game: String,
        mapping: Mapping,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        self.location_command(sender, async move {
            let result = tracker
                .group_trilogy_profiles(&game, &mapping)
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:#}"));
            AppCmdMsg::Games(GamesCmdMsg::TrilogyGrouped(game, result))
        });
    }
}

#[derive(Debug, PartialEq, Eq)]
enum GameSection {
    Single(usize),
    Trilogy(Vec<usize>),
}

fn game_sections(games: &[crate::models::game::Game]) -> Vec<GameSection> {
    let mut trilogy: Vec<_> = games
        .iter()
        .enumerate()
        .filter(|(_, game)| game.engine == crate::models::game::GameEngine::MassEffect)
        .map(|(index, _)| index)
        .collect();
    trilogy.sort_by_key(|index| &games[*index].id);
    let mut sections = Vec::new();
    let mut added = false;
    for (index, game) in games.iter().enumerate() {
        if game.engine == crate::models::game::GameEngine::MassEffect {
            if !added {
                sections.push(GameSection::Trilogy(trilogy.clone()));
                added = true;
            }
        } else {
            sections.push(GameSection::Single(index));
        }
    }
    sections
}

impl App {
    pub(crate) fn refresh_grouped_game_picker(&self) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        content.set_margin_top(6);
        content.set_margin_bottom(6);
        content.set_margin_start(6);
        content.set_margin_end(6);
        let popover = gtk::Popover::new();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(500)
            .child(&content)
            .build();
        popover.set_child(Some(&scroller));
        let add_game = |parent: &gtk::Box, index: usize| {
            let game = &self.session.games[index];
            let button = gtk::Button::with_label(&game.title);
            button.add_css_class("flat");
            if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                label.set_xalign(0.0);
            }
            let selection = self.ui.game_selection.clone();
            let popover = popover.downgrade();
            button.connect_clicked(move |_| {
                if let Some(popover) = popover.upgrade() {
                    popover.popdown();
                }
                selection.set_selected(index as u32);
            });
            parent.append(&button);
        };
        for section in game_sections(&self.session.games) {
            match section {
                GameSection::Single(index) => add_game(&content, index),
                GameSection::Trilogy(indices) => {
                    let members = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    members.set_margin_start(18);
                    for index in &indices {
                        add_game(&members, *index);
                    }
                    let group = gtk::Expander::builder()
                        .label("Mass Effect Legendary Edition")
                        .expanded(indices.contains(&self.session.selected_game_idx))
                        .child(&members)
                        .build();
                    content.append(&group);
                }
            }
        }
        let title = self
            .selected_game()
            .map(|game| game.title.as_str())
            .unwrap_or("Choose game");
        let label = gtk::Label::builder()
            .label(title)
            .max_width_chars(25)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let face = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        face.append(&label);
        face.append(&gtk::Image::from_icon_name("pan-down-symbolic"));
        self.ui.game_picker.set_child(Some(&face));
        self.ui.game_picker.set_popover(Some(&popover));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::game::{Game, GameEngine};

    // @variants: both
    #[test]
    fn groups_interleaved_trilogy_members_without_changing_selection_indices() {
        let games = [
            ("other", GameEngine::Bethesda),
            ("mass-effect-le3", GameEngine::MassEffect),
            ("another", GameEngine::Bethesda),
            ("mass-effect-le1", GameEngine::MassEffect),
            ("mass-effect-le2", GameEngine::MassEffect),
        ]
        .into_iter()
        .map(|(id, engine)| Game {
            id: id.into(),
            title: id.into(),
            engine,
            path: "/game".into(),
            data_subdir: "Data".into(),
            wine_prefix: None,
        })
        .collect::<Vec<_>>();
        assert_eq!(
            game_sections(&games),
            vec![
                GameSection::Single(0),
                GameSection::Trilogy(vec![3, 4, 1]),
                GameSection::Single(2)
            ]
        );
        assert!(game_sections(&[]).is_empty());
    }
}
