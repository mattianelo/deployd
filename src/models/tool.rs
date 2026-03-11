/// An external Windows tool (e.g. xEdit, Outfit Studio) configured for a specific game.
#[derive(Debug, Clone)]
pub struct Tool {
    pub id: String,
    pub game_id: String,
    pub name: String,
    /// Absolute Linux filesystem path to the Windows .exe file.
    pub exe_path: String,
    /// GTK symbolic icon name for the headerbar button.
    pub icon_name: String,
    /// Additional command-line arguments passed to the tool (space-separated).
    pub custom_args: String,
    /// Display ordering in the headerbar (lower = leftmost).
    pub sort_order: i32,
    /// Working directory (Linux path) passed as CWD to Wine when launching.
    /// Empty string means use the exe's parent directory.
    pub working_dir: String,
}
