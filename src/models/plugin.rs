#[derive(Debug, Clone)]
pub struct Plugin {
    pub id: String,
    pub mod_id: String,
    pub filename: String,
    pub load_order: i32,
    pub enabled: bool,
}

/// Dirty-edit summary from the LOOT masterlist for a single plugin.
/// Counts are 0 when the masterlist doesn't record that category.
#[derive(Debug, Clone)]
pub struct PluginDirtyInfo {
    /// Identical-To-Master record count.
    pub itm: u32,
    /// Undeleted (deleted reference) count.
    pub udr: u32,
    /// Deleted navmesh count.
    pub nav: u32,
    /// Cleaning utility name as reported by the masterlist (e.g. "SSEEdit 4.1.3b").
    pub cleaning_utility: String,
}

impl PluginDirtyInfo {
    /// Build a human-readable tooltip string for this dirty-edit entry.
    pub fn tooltip(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.itm > 0 {
            parts.push(format!("{} ITM", self.itm));
        }
        if self.udr > 0 {
            parts.push(format!("{} UDR", self.udr));
        }
        if self.nav > 0 {
            parts.push(format!("{} deleted navmesh(es)", self.nav));
        }
        let counts = if parts.is_empty() {
            "Dirty edits detected".to_string()
        } else {
            parts.join(", ")
        };
        format!("{counts} — clean with {}", self.cleaning_utility)
    }
}
