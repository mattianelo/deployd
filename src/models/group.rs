#[derive(Debug, Clone)]
pub struct ModGroup {
    pub id: String,
    pub name: String,
    /// Position in the combined (groups + mods) sorted list.
    /// Stored as f64 so new groups can be inserted between existing ones without renumbering.
    pub position: f64,
    pub collapsed: bool,
}
