#[derive(Debug, Clone)]
pub struct ModGroup {
    pub id: String,
    pub name: String,
    /// Position in the combined (groups + mods) sorted list.
    /// Stored as f64 so new groups can be inserted between existing ones without renumbering.
    pub position: f64,
    pub collapsed: bool,
    /// Optional color label (e.g. "red", "blue"). Drives the color dot in the group header.
    pub color: Option<String>,
}
