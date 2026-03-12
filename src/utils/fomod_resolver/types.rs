use std::collections::{HashMap, HashSet};

/// A single file mapping resolved from a FOMOD config.
#[derive(Debug)]
pub struct FomodFileMapping {
    /// Path relative to the extracted root.
    pub source_relative: std::path::PathBuf,
    /// Destination path relative to the game data directory.
    pub dest_relative: std::path::PathBuf,
}

/// Parsed FOMOD config ready for UI display.
#[derive(Debug, Clone)]
pub struct FomodUiConfig {
    pub steps: Vec<FomodUiStep>,
}

#[derive(Debug, Clone)]
pub struct FomodUiStep {
    pub name: String,
    pub groups: Vec<FomodUiGroup>,
    /// Visibility conditions — if present, step is only shown when these are satisfied.
    pub visible: Option<FomodUiDependencies>,
}

/// A dependency tree for evaluating visibility conditions.
#[derive(Debug, Clone)]
pub struct FomodUiDependencies {
    pub operator: String,
    pub flag_deps: Vec<(String, String)>,
    pub nested: Vec<FomodUiDependencies>,
}

impl FomodUiDependencies {
    /// Evaluate this dependency tree against accumulated condition flags.
    pub fn evaluate(&self, flags: &HashMap<String, String>) -> bool {
        let is_and = self.operator.eq_ignore_ascii_case("and");
        let mut results: Vec<bool> = Vec::new();

        for (name, value) in &self.flag_deps {
            results.push(flags.get(name).is_some_and(|v| v == value));
        }
        for nested in &self.nested {
            results.push(nested.evaluate(flags));
        }

        if results.is_empty() {
            return is_and;
        }
        if is_and {
            results.iter().all(|&r| r)
        } else {
            results.iter().any(|&r| r)
        }
    }
}

#[derive(Debug, Clone)]
pub struct FomodUiGroup {
    pub name: String,
    pub group_type: FomodGroupType,
    pub plugins: Vec<FomodUiPlugin>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FomodGroupType {
    SelectAll,
    SelectExactlyOne,
    SelectAtLeastOne,
    SelectAtMostOne,
    SelectAny,
}

#[derive(Debug, Clone)]
pub struct FomodUiPlugin {
    pub name: String,
    pub description: String,
    /// "Required", "Recommended", "Optional", "NotUsable", or ""
    pub type_hint: String,
    /// Condition flags set when this plugin is selected: (name, value).
    pub condition_flags: Vec<(String, String)>,
}

/// User selections from the FOMOD wizard.
/// `selections[step_idx][group_idx]` = set of selected plugin indices.
#[derive(Debug, Clone)]
pub struct FomodSelections {
    pub selections: Vec<Vec<HashSet<usize>>>,
    /// Accumulated condition flags from all selected plugins.
    pub flags: HashMap<String, String>,
}
