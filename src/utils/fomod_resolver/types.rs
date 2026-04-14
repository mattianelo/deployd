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
    /// File dependencies: (lowercased file path, required state e.g. "Active"/"Missing").
    pub file_deps: Vec<(String, String)>,
    pub nested: Vec<FomodUiDependencies>,
}

impl FomodUiDependencies {
    /// Evaluate against condition flags only (no file context).
    pub fn evaluate(&self, flags: &HashMap<String, String>) -> bool {
        self.evaluate_with_files(flags, &HashSet::new())
    }

    /// Evaluate against condition flags and a set of known active file names (lowercased).
    pub fn evaluate_with_files(
        &self,
        flags: &HashMap<String, String>,
        files: &HashSet<String>,
    ) -> bool {
        let is_and = self.operator.eq_ignore_ascii_case("and");
        let mut results: Vec<bool> = Vec::new();

        for (name, value) in &self.flag_deps {
            results.push(flags.get(name).is_some_and(|v| v == value));
        }
        for (file, state) in &self.file_deps {
            let present = files.contains(file.as_str());
            let satisfied = match state.as_str() {
                "Active" => present,
                "Missing" => !present,
                _ => present,
            };
            results.push(satisfied);
        }
        for nested in &self.nested {
            results.push(nested.evaluate_with_files(flags, files));
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
#[allow(clippy::enum_variant_names)] // Variant names mirror the FOMOD XML spec values verbatim.
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
    /// Optional image path (relative to archive root, may use Windows backslashes).
    pub image_path: Option<String>,
    /// Fallback type from `<dependencyType><defaultType>` (empty when absent).
    pub dep_type_default: String,
    /// Conditional type patterns: (deps, type_name). First match wins.
    pub dep_type_patterns: Vec<(FomodUiDependencies, String)>,
}

/// User selections from the FOMOD wizard.
/// `selections[step_idx][group_idx]` = set of selected plugin indices.
#[derive(Debug, Clone)]
pub struct FomodSelections {
    pub selections: Vec<Vec<HashSet<usize>>>,
    /// Accumulated condition flags from all selected plugins.
    pub flags: HashMap<String, String>,
}
