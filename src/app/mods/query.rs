use crate::ui::mod_list::ModListItemKind;

use super::super::App;

impl App {
    /// Return `(mod_id, priority)` for an existing mod whose name matches `name`
    /// (case-insensitive), or `None` if not found.
    pub(crate) fn find_mod_id_and_priority_by_name(&self, name: &str) -> Option<(String, i32)> {
        self.mods.rows.iter().find_map(|item| {
            if let ModListItemKind::Mod(ref m) = item.kind
                && m.mod_entry.name.eq_ignore_ascii_case(name)
            {
                Some((m.mod_entry.id.clone(), m.mod_entry.priority))
            } else {
                None
            }
        })
    }

    /// Return the display name for a mod by its ID, or the ID itself if not found.
    pub(crate) fn mod_name_for_id(&self, mod_id: &str) -> String {
        self.mods
            .rows
            .iter()
            .find_map(|item| {
                if let ModListItemKind::Mod(ref m) = item.kind
                    && m.mod_entry.id == mod_id
                {
                    return Some(m.mod_entry.name.clone());
                }
                None
            })
            .unwrap_or_else(|| mod_id.to_string())
    }
}
