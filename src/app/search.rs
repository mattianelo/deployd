use crate::models::download::{DownloadFilter, DownloadStatus};

use super::App;
use super::types::{ModFilter, SearchScope};

impl App {
    pub(crate) fn apply_search_filter(&mut self) {
        self.rebuild_override_panel();
        let query = self.shell.search_text.to_lowercase();
        let empty = query.is_empty();

        {
            let searches_mods = matches!(
                self.shell.search_scope,
                SearchScope::All | SearchScope::ModOrder
            );
            let dao = self.game_shows_overrides();
            let mod_filter = self.mods.filter;
            let no_filter = (!searches_mods || empty) && matches!(mod_filter, ModFilter::All);
            let mut in_collapsed_group = false;
            let mut guard = self.mods.rows.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    if row.is_separator() {
                        in_collapsed_group = row.is_collapsed();
                        row.visible = no_filter && !dao;
                    } else if dao
                        && row
                            .mod_id()
                            .is_some_and(|id| self.ui.override_mod_ids.contains(id))
                    {
                        row.visible = false;
                    } else if no_filter {
                        row.visible = dao || !in_collapsed_group;
                    } else {
                        let name_match = !searches_mods || empty || row.matches_search(&query);
                        let filter_match = match mod_filter {
                            ModFilter::All => true,
                            ModFilter::Enabled => {
                                row.mod_row().is_some_and(|r| r.mod_entry.enabled)
                            }
                            ModFilter::Issues => row
                                .mod_row()
                                .is_some_and(|r| r.overrides > 0 || r.overridden_by > 0),
                        };
                        row.visible = name_match && filter_match;
                    }
                }
            }
        }

        if self.shell.search_scope == SearchScope::All
            || self.shell.search_scope == SearchScope::PluginOrder
        {
            let mut guard = self.plugins.rows.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    row.visible = empty || row.search_key.contains(&query);
                }
            }
        }

        if self.shell.search_scope == SearchScope::All
            || self.shell.search_scope == SearchScope::Downloads
        {
            let download_filter = self.download.filter;
            let mut guard = self.download.rows.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    let name_match = empty || row.entry.mod_name.to_lowercase().contains(&query);
                    let filter_match = match download_filter {
                        DownloadFilter::All => true,
                        DownloadFilter::Active => row.entry.is_active(),
                        DownloadFilter::Completed => matches!(
                            row.entry.status,
                            DownloadStatus::Installed | DownloadStatus::Downloaded
                        ),
                    };
                    row.visible = name_match && filter_match;
                }
            }
        }
    }
}
