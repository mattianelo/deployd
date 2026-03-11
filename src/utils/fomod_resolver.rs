use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use walkdir::WalkDir;

use crate::dlog;

// ---------------------------------------------------------------------------
// Public types for FOMOD UI
// ---------------------------------------------------------------------------

/// A single file mapping resolved from a FOMOD config.
#[derive(Debug)]
pub struct FomodFileMapping {
    /// Path relative to the extracted root.
    pub source_relative: PathBuf,
    /// Destination path relative to the game data directory.
    pub dest_relative: PathBuf,
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

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Look for fomod/ModuleConfig.xml (case-insensitive) in the extracted directory.
/// Searches up to depth 5 to handle mods with extra wrapper directories.
/// Returns the absolute path to the config if found.
pub fn detect_fomod(extracted_root: &Path) -> Option<PathBuf> {
    let mut info_xml_found = false;

    for entry in WalkDir::new(extracted_root).max_depth(5) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().is_file() {
            let rel = entry
                .path()
                .strip_prefix(extracted_root)
                .unwrap_or(entry.path());
            let rel_lower = rel.to_string_lossy().to_lowercase().replace('\\', "/");
            if rel_lower.ends_with("fomod/moduleconfig.xml") {
                return Some(entry.path().to_path_buf());
            }
            if rel_lower.ends_with("fomod/info.xml") {
                info_xml_found = true;
            }
        }
    }

    if info_xml_found {
        dlog!("[deployd] Found fomod/info.xml but no ModuleConfig.xml — FOMOD may be incomplete");
    }

    None
}

// ---------------------------------------------------------------------------
// Parsing for UI
// ---------------------------------------------------------------------------

/// Parse a FOMOD ModuleConfig.xml into a UI-friendly structure.
pub fn parse_fomod_config(config_path: &Path) -> Result<FomodUiConfig> {
    let xml = read_fomod_xml(config_path)?;

    let config: XmlConfig =
        quick_xml::de::from_str(&xml).map_err(|e| anyhow::anyhow!("FOMOD parse error: {e}"))?;

    let steps = config
        .install_steps
        .as_ref()
        .map(|step_list| {
            step_list
                .install_step
                .iter()
                .map(|step| FomodUiStep {
                    name: step.name.clone(),
                    visible: step.visible.as_ref().map(convert_deps_to_ui),
                    groups: step
                        .optional_file_groups
                        .as_ref()
                        .map(|gl| {
                            gl.group
                                .iter()
                                .map(|g| FomodUiGroup {
                                    name: g.name.clone(),
                                    group_type: parse_group_type(&g.typ),
                                    plugins: g
                                        .plugins
                                        .as_ref()
                                        .map(|pl| {
                                            pl.plugin
                                                .iter()
                                                .map(|p| FomodUiPlugin {
                                                    name: p.name.clone(),
                                                    description: p
                                                        .description
                                                        .as_ref()
                                                        .map(|d| d.text.clone())
                                                        .unwrap_or_default(),
                                                    type_hint: p
                                                        .type_descriptor
                                                        .as_ref()
                                                        .and_then(|td| td.typ.as_ref())
                                                        .map(|t| t.name.clone())
                                                        .unwrap_or_default(),
                                                    condition_flags: p
                                                        .condition_flags
                                                        .as_ref()
                                                        .map(|cf| {
                                                            cf.flags
                                                                .iter()
                                                                .map(|f| {
                                                                    (
                                                                        f.name.clone(),
                                                                        f.value.clone(),
                                                                    )
                                                                })
                                                                .collect()
                                                        })
                                                        .unwrap_or_default(),
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(FomodUiConfig { steps })
}

/// Returns true if the FOMOD config has at least one step with a group that requires user input.
/// Used to decide whether to show the installer dialog or auto-install with defaults.
pub fn needs_user_input(config: &FomodUiConfig) -> bool {
    config.steps.iter().any(|step| {
        step.groups.iter().any(|g| {
            if g.plugins.is_empty() {
                return false;
            }
            match g.group_type {
                FomodGroupType::SelectAll => false,
                FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtLeastOne => {
                    g.plugins.len() > 1
                }
                FomodGroupType::SelectAtMostOne | FomodGroupType::SelectAny => true,
            }
        })
    })
}

fn convert_deps_to_ui(deps: &XmlDependencies) -> FomodUiDependencies {
    FomodUiDependencies {
        operator: deps.operator.clone(),
        flag_deps: deps
            .flag_dependencies
            .iter()
            .map(|f| (f.flag.clone(), f.value.clone()))
            .collect(),
        nested: deps.nested.iter().map(convert_deps_to_ui).collect(),
    }
}

fn parse_group_type(s: &str) -> FomodGroupType {
    match s {
        "SelectAll" => FomodGroupType::SelectAll,
        "SelectExactlyOne" => FomodGroupType::SelectExactlyOne,
        "SelectAtLeastOne" => FomodGroupType::SelectAtLeastOne,
        "SelectAtMostOne" => FomodGroupType::SelectAtMostOne,
        "SelectAny" => FomodGroupType::SelectAny,
        _ => FomodGroupType::SelectAny,
    }
}

// ---------------------------------------------------------------------------
// Resolution with user selections
// ---------------------------------------------------------------------------

/// Resolve files to install based on user selections from the FOMOD wizard.
pub fn resolve_fomod_with_selections(
    config_path: &Path,
    extracted_root: &Path,
    selections: &FomodSelections,
) -> Result<Vec<FomodFileMapping>> {
    let xml = read_fomod_xml(config_path)?;

    let config: XmlConfig =
        quick_xml::de::from_str(&xml).map_err(|e| anyhow::anyhow!("FOMOD parse error: {e}"))?;

    // Content root = parent of fomod/ directory (handles wrapper directories)
    let content_root = config_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(extracted_root);
    let path_index = build_path_index(extracted_root, content_root);
    let mut mappings = Vec::new();
    let mut warnings = Vec::new();

    // Always include required install files
    if let Some(ref req) = config.required_install_files {
        for entry in &req.items {
            collect_file(
                &entry.attrs(),
                extracted_root,
                &path_index,
                &mut mappings,
                &mut warnings,
            )?;
        }
    }

    // Walk steps/groups, respecting step visibility and only installing files for
    // plugins selected by the user. Flags are accumulated incrementally so that
    // each step's visibility is evaluated with the correct prior-step flags.
    let mut step_flags: HashMap<String, String> = HashMap::new();
    if let Some(ref steps) = config.install_steps {
        for (step_idx, step) in steps.install_step.iter().enumerate() {
            // Skip steps whose visibility condition is not satisfied.
            let step_visible = step
                .visible
                .as_ref()
                .map(|v| evaluate_dependencies(v, &step_flags))
                .unwrap_or(true);
            if !step_visible {
                continue;
            }

            if let Some(ref groups) = step.optional_file_groups {
                for (group_idx, group) in groups.group.iter().enumerate() {
                    let selected_indices = selections
                        .selections
                        .get(step_idx)
                        .and_then(|step_sel| step_sel.get(group_idx));

                    if let (Some(selected), Some(pl)) = (selected_indices, &group.plugins) {
                        for (plugin_idx, plugin) in pl.plugin.iter().enumerate() {
                            if selected.contains(&plugin_idx) {
                                if let Some(ref files) = plugin.files {
                                    for entry in &files.items {
                                        collect_file(
                                            &entry.attrs(),
                                            extracted_root,
                                            &path_index,
                                            &mut mappings,
                                            &mut warnings,
                                        )?;
                                    }
                                }
                                // Accumulate flags for subsequent step visibility checks.
                                if let Some(ref cf) = plugin.condition_flags {
                                    for flag in &cf.flags {
                                        step_flags.insert(flag.name.clone(), flag.value.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Evaluate conditionalFileInstalls using only flags from visible/selected steps.
    process_conditional_installs(
        &config,
        extracted_root,
        &path_index,
        &step_flags,
        &mut mappings,
        &mut warnings,
    )?;

    for w in &warnings {
        eprintln!("[deployd] {w}");
    }

    // Defensive: expand any directory sources that somehow made it into mappings.
    // collect_file() is supposed to expand directories via WalkDir, but malformed FOMOD
    // configs or edge cases (e.g. back.dds is a dir in the archive but the XML lists it as
    // a <file>) can result in a directory path reaching the installer.
    let mappings = expand_directory_sources(mappings, extracted_root)?;

    Ok(mappings)
}

// ---------------------------------------------------------------------------
// Default resolution (no UI)
// ---------------------------------------------------------------------------

/// Parse a FOMOD ModuleConfig.xml and resolve a default set of files to install.
///
/// Strategy (no UI):
/// 1. Always include `required_install_files`.
/// 2. Walk install steps → groups → auto-pick plugins
///    (prefer Recommended/Required, else first in group).
/// 3. Evaluate conditionalFileInstalls using flags from default-selected plugins.
pub fn resolve_fomod_default(
    extracted_root: &Path,
    config_path: &Path,
) -> Result<Vec<FomodFileMapping>> {
    let xml = read_fomod_xml(config_path)?;

    let config: XmlConfig =
        quick_xml::de::from_str(&xml).map_err(|e| anyhow::anyhow!("FOMOD parse error: {e}"))?;

    // Content root = parent of fomod/ directory (handles wrapper directories)
    let content_root = config_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(extracted_root);
    let path_index = build_path_index(extracted_root, content_root);
    let mut mappings = Vec::new();
    let mut warnings = Vec::new();

    // 1. Required install files
    if let Some(ref req) = config.required_install_files {
        for entry in &req.items {
            collect_file(
                &entry.attrs(),
                extracted_root,
                &path_index,
                &mut mappings,
                &mut warnings,
            )?;
        }
    }

    // 2. Install steps → groups → default plugin selection
    // Also collect condition flags from selected plugins for step 3.
    let mut flags = HashMap::new();

    if let Some(ref steps) = config.install_steps {
        for step in &steps.install_step {
            // Respect step visibility: skip steps whose condition is not satisfied.
            let step_visible = step
                .visible
                .as_ref()
                .map(|v| evaluate_dependencies(v, &flags))
                .unwrap_or(true);
            if !step_visible {
                continue;
            }

            if let Some(ref groups) = step.optional_file_groups {
                for group in &groups.group {
                    let selected = pick_default_plugins(group);
                    for plugin in &selected {
                        if let Some(ref files) = plugin.files {
                            for entry in &files.items {
                                collect_file(
                                    &entry.attrs(),
                                    extracted_root,
                                    &path_index,
                                    &mut mappings,
                                    &mut warnings,
                                )?;
                            }
                        }
                        // Accumulate condition flags
                        if let Some(ref cf) = plugin.condition_flags {
                            for flag in &cf.flags {
                                flags.insert(flag.name.clone(), flag.value.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Evaluate conditionalFileInstalls
    process_conditional_installs(
        &config,
        extracted_root,
        &path_index,
        &flags,
        &mut mappings,
        &mut warnings,
    )?;

    for w in &warnings {
        eprintln!("[deployd] {w}");
    }

    Ok(mappings)
}

// ---------------------------------------------------------------------------
// Case-insensitive path index
// ---------------------------------------------------------------------------

/// Build a lookup table mapping lowercased relative paths to real on-disk paths.
/// Covers all files and directories in the extracted tree.
///
/// Keys are added relative to `extracted_root`.  When `content_root` differs
/// (i.e. the archive has a wrapper directory), additional keys relative to
/// `content_root` are added so FOMOD source paths work regardless of whether
/// they include the wrapper prefix.
fn build_path_index(extracted_root: &Path, content_root: &Path) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    for entry in WalkDir::new(extracted_root) {
        let Ok(entry) = entry else { continue };
        let abs = entry.path().to_path_buf();

        // Key relative to extracted_root (archive root)
        if let Ok(rel) = entry.path().strip_prefix(extracted_root) {
            let key = rel.to_string_lossy().to_lowercase().replace('\\', "/");
            if !key.is_empty() {
                index.insert(key, abs.clone());
            }
        }

        // Also index relative to content_root for mods with wrapper directories
        if content_root != extracted_root
            && let Ok(rel) = entry.path().strip_prefix(content_root)
        {
            let key = rel.to_string_lossy().to_lowercase().replace('\\', "/");
            if !key.is_empty() {
                index.entry(key).or_insert(abs);
            }
        }
    }
    index
}

// ---------------------------------------------------------------------------
// Conditional file installs
// ---------------------------------------------------------------------------

/// Process `<conditionalFileInstalls>` from the FOMOD config.
fn process_conditional_installs(
    config: &XmlConfig,
    extracted_root: &Path,
    path_index: &HashMap<String, PathBuf>,
    flags: &HashMap<String, String>,
    mappings: &mut Vec<FomodFileMapping>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let Some(ref cfi) = config.conditional_file_installs else {
        return Ok(());
    };
    let Some(ref patterns) = cfi.patterns else {
        return Ok(());
    };
    for pattern in &patterns.pattern {
        let satisfied = match &pattern.dependencies {
            Some(deps) => evaluate_dependencies(deps, flags),
            None => true,
        };
        if satisfied && let Some(ref files) = pattern.files {
            for entry in &files.items {
                collect_file(
                    &entry.attrs(),
                    extracted_root,
                    path_index,
                    mappings,
                    warnings,
                )?;
            }
        }
    }
    Ok(())
}

/// Walk any FomodFileMapping whose source_relative points to a directory and expand it
/// into individual file mappings. Non-directory sources pass through unchanged.
///
/// This is a safety net for FOMOD configs that reference a directory as a `<file>` source
/// (which technically should be `<folder>`, but is common in the wild). The collect_file
/// function handles this via the `is_dir()` branch, but this expansion catches any
/// edge cases that slip through (e.g. a directory that somehow passes is_file() on some
/// unusual extract path).
fn expand_directory_sources(
    mappings: Vec<FomodFileMapping>,
    extracted_root: &Path,
) -> Result<Vec<FomodFileMapping>> {
    let mut expanded = Vec::with_capacity(mappings.len());
    for m in mappings {
        let abs = extracted_root.join(&m.source_relative);
        if abs.is_dir() {
            dlog!(
                "[deployd] FOMOD: expanding directory source '{}' → dest '{}'",
                m.source_relative.display(),
                m.dest_relative.display()
            );
            let mut file_count = 0usize;
            for entry in WalkDir::new(&abs) {
                let entry = entry?;
                if entry.path() == abs || !entry.file_type().is_file() {
                    continue;
                }
                let rel = entry.path().strip_prefix(&abs)?;
                expanded.push(FomodFileMapping {
                    source_relative: entry.path().strip_prefix(extracted_root)?.to_path_buf(),
                    dest_relative: m.dest_relative.join(rel),
                });
                file_count += 1;
            }
            if file_count == 0 {
                dlog!(
                    "[deployd] FOMOD: expanded directory '{}' has no files — adding sentinel",
                    m.source_relative.display()
                );
                // Emit a directory sentinel so the game folder gets created.
                expanded.push(FomodFileMapping {
                    source_relative: m.source_relative.clone(),
                    dest_relative: m.dest_relative.clone(),
                });
            }
        } else if !abs.is_file() {
            // Source is neither a regular file nor a directory (broken symlink, special
            // device, or a path that changed between resolution and expansion). Skip it
            // rather than handing a bad path to the installer.
            dlog!(
                "[deployd] FOMOD: source '{}' is not a regular file — skipping",
                abs.display()
            );
        } else {
            expanded.push(m);
        }
    }
    Ok(expanded)
}

/// Evaluate a dependency tree against accumulated condition flags.
fn evaluate_dependencies(deps: &XmlDependencies, flags: &HashMap<String, String>) -> bool {
    let is_and = deps.operator.eq_ignore_ascii_case("and");

    let mut results: Vec<bool> = Vec::new();

    for fd in &deps.flag_dependencies {
        let matched = flags.get(&fd.flag).is_some_and(|v| v == &fd.value);
        results.push(matched);
    }

    // File dependencies: treat as NOT satisfied — we have no access to the game
    // directory at install time, so we conservatively assume the file is absent.
    // Mods that check for DLC ESMs (e.g. Dawnguard.esm) will therefore not
    // install their optional DLC patches automatically, which is the safe default.
    for _fd in &deps.file_dependencies {
        results.push(false);
    }

    // Recursive nested dependencies
    for nested in &deps.nested {
        results.push(evaluate_dependencies(nested, flags));
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

// ---------------------------------------------------------------------------
// Lenient XML types — serde deserialization via quick-xml
//
// Key differences from the upstream `fomod` crate:
// - No enums for dependency types → avoids "unknown variant" errors
// - All dependency/visibility fields simply omitted → silently skipped
// - Group type is a plain String with default → handles unknown types
// - operator is never required → fixes "missing field 'operator'"
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct XmlConfig {
    #[serde(rename = "requiredInstallFiles")]
    required_install_files: Option<XmlFileList>,

    #[serde(rename = "installSteps")]
    install_steps: Option<XmlStepList>,

    #[serde(rename = "conditionalFileInstalls")]
    conditional_file_installs: Option<XmlConditionalInstalls>,
    // moduleName, moduleImage, moduleDependencies → silently ignored.
}

#[derive(Debug, Deserialize)]
enum XmlFileEntry {
    #[serde(rename = "file")]
    File(XmlFileAttrs),
    #[serde(rename = "folder")]
    Folder(XmlFileAttrs),
}

impl XmlFileEntry {
    fn attrs(&self) -> FileRef<'_> {
        match self {
            XmlFileEntry::File(a) | XmlFileEntry::Folder(a) => FileRef {
                source: &a.source,
                destination: a.destination.as_deref(),
            },
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct XmlFileList {
    #[serde(rename = "$value", default)]
    items: Vec<XmlFileEntry>,
}

#[derive(Debug, Deserialize)]
struct XmlFileAttrs {
    #[serde(rename = "@source")]
    source: String,
    #[serde(rename = "@destination")]
    destination: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XmlStepList {
    #[serde(rename = "installStep", default)]
    install_step: Vec<XmlInstallStep>,
}

#[derive(Debug, Deserialize)]
struct XmlInstallStep {
    #[serde(rename = "@name", default)]
    name: String,

    #[serde(rename = "optionalFileGroups")]
    optional_file_groups: Option<XmlGroupList>,

    /// Visibility conditions — step is only shown when these are satisfied.
    visible: Option<XmlDependencies>,
}

#[derive(Debug, Deserialize)]
struct XmlGroupList {
    #[serde(rename = "group", default)]
    group: Vec<XmlGroup>,
}

#[derive(Debug, Deserialize)]
struct XmlGroup {
    #[serde(rename = "@name", default)]
    name: String,

    #[serde(rename = "@type", default)]
    typ: String,

    plugins: Option<XmlPluginList>,
}

#[derive(Debug, Deserialize)]
struct XmlPluginList {
    #[serde(rename = "plugin", default)]
    plugin: Vec<XmlPlugin>,
}

#[derive(Debug, Deserialize)]
struct XmlPlugin {
    #[serde(rename = "@name", default)]
    name: String,

    description: Option<XmlDescription>,

    files: Option<XmlFileList>,

    #[serde(rename = "typeDescriptor")]
    type_descriptor: Option<XmlTypeDescriptor>,

    #[serde(rename = "conditionFlags")]
    condition_flags: Option<XmlConditionFlags>,
    // image → silently ignored.
}

#[derive(Debug, Deserialize)]
struct XmlDescription {
    #[serde(rename = "$text", default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct XmlTypeDescriptor {
    #[serde(rename = "type")]
    typ: Option<XmlPluginType>,
    // dependencyType → silently ignored.
}

#[derive(Debug, Deserialize)]
struct XmlPluginType {
    #[serde(rename = "@name", default)]
    name: String,
}

// ---------------------------------------------------------------------------
// Condition flags and conditional file installs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct XmlConditionFlags {
    #[serde(rename = "flag", default)]
    flags: Vec<XmlFlag>,
}

#[derive(Debug, Deserialize)]
struct XmlFlag {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "$text", default)]
    value: String,
}

#[derive(Debug, Deserialize, Default)]
struct XmlConditionalInstalls {
    patterns: Option<XmlConditionalPatterns>,
}

#[derive(Debug, Deserialize, Default)]
struct XmlConditionalPatterns {
    #[serde(rename = "pattern", default)]
    pattern: Vec<XmlConditionalPattern>,
}

#[derive(Debug, Deserialize)]
struct XmlConditionalPattern {
    dependencies: Option<XmlDependencies>,
    files: Option<XmlFileList>,
}

#[derive(Debug, Deserialize)]
struct XmlDependencies {
    #[serde(rename = "@operator", default = "default_operator_and")]
    operator: String,

    #[serde(rename = "flagDependency", default)]
    flag_dependencies: Vec<XmlFlagDependency>,

    #[serde(rename = "fileDependency", default)]
    file_dependencies: Vec<XmlFileDependency>,

    /// Nested composite dependencies (recursive AND/OR).
    #[serde(rename = "dependencies", default)]
    nested: Vec<XmlDependencies>,
}

fn default_operator_and() -> String {
    "And".to_string()
}

#[derive(Debug, Deserialize)]
struct XmlFlagDependency {
    #[serde(rename = "@flag")]
    flag: String,
    #[serde(rename = "@value")]
    value: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct XmlFileDependency {
    #[serde(rename = "@file")]
    file: String,
    #[serde(rename = "@state")]
    state: String,
}

// ---------------------------------------------------------------------------
// Plugin selection (default)
// ---------------------------------------------------------------------------

fn pick_default_plugins(group: &XmlGroup) -> Vec<&XmlPlugin> {
    let plugins = match &group.plugins {
        Some(pl) => &pl.plugin,
        None => return vec![],
    };

    match group.typ.as_str() {
        "SelectAll" => plugins.iter().collect(),
        "SelectExactlyOne" | "SelectAtLeastOne" => {
            let rec = find_recommended(plugins);
            if rec.is_empty() {
                plugins.iter().take(1).collect()
            } else {
                rec
            }
        }
        "SelectAtMostOne" | "SelectAny" => {
            let rec = find_recommended(plugins);
            if rec.is_empty() {
                plugins.iter().take(1).collect()
            } else {
                rec
            }
        }
        _ => {
            // Unknown group type — try recommended, fall back to first.
            let rec = find_recommended(plugins);
            if rec.is_empty() {
                plugins.iter().take(1).collect()
            } else {
                rec
            }
        }
    }
}

fn find_recommended(plugins: &[XmlPlugin]) -> Vec<&XmlPlugin> {
    plugins
        .iter()
        .filter(|p| {
            p.type_descriptor
                .as_ref()
                .and_then(|td| td.typ.as_ref())
                .is_some_and(|t| t.name == "Recommended" || t.name == "Required")
        })
        .collect()
}

// ---------------------------------------------------------------------------
// File collection
// ---------------------------------------------------------------------------

struct FileRef<'a> {
    source: &'a str,
    destination: Option<&'a str>,
}

/// Read FOMOD XML with BOM and encoding handling.
/// Supports UTF-8 (with/without BOM), UTF-16LE, and UTF-16BE.
fn read_fomod_xml(config_path: &Path) -> Result<String> {
    let raw = std::fs::read(config_path)
        .with_context(|| format!("Cannot read FOMOD config: {}", config_path.display()))?;

    let text = if raw.starts_with(&[0xFF, 0xFE]) {
        // UTF-16LE BOM
        decode_utf16le(&raw[2..])
    } else if raw.starts_with(&[0xFE, 0xFF]) {
        // UTF-16BE BOM
        decode_utf16be(&raw[2..])
    } else if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        // UTF-8 BOM
        String::from_utf8_lossy(&raw[3..]).into_owned()
    } else if looks_utf16le(&raw) {
        // UTF-16LE without BOM (detected by null byte pattern)
        decode_utf16le(&raw)
    } else {
        String::from_utf8_lossy(&raw).into_owned()
    };

    Ok(text)
}

fn decode_utf16le(data: &[u8]) -> String {
    let iter = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    char::decode_utf16(iter)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn decode_utf16be(data: &[u8]) -> String {
    let iter = data
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
    char::decode_utf16(iter)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Heuristic: if the file has even length and every other byte is 0x00 in the
/// first few bytes, it's likely UTF-16LE without BOM.
fn looks_utf16le(data: &[u8]) -> bool {
    if data.len() < 4 || !data.len().is_multiple_of(2) {
        return false;
    }
    // Check first 8 byte pairs: in UTF-16LE ASCII, odd bytes are 0x00
    let check_len = data.len().min(16);
    let null_count = data[1..check_len]
        .iter()
        .step_by(2)
        .filter(|&&b| b == 0)
        .count();
    null_count >= 3
}

/// Collect files from a single file/folder entry using case-insensitive path lookup.
/// If the source is a directory, recursively add all files inside it.
fn collect_file(
    ft: &FileRef<'_>,
    extracted_root: &Path,
    path_index: &HashMap<String, PathBuf>,
    mappings: &mut Vec<FomodFileMapping>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let source_key = ft
        .source
        .to_lowercase()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();

    let source_path = match path_index.get(&source_key) {
        Some(p) => p.clone(),
        None => {
            if !source_key.is_empty() {
                warnings.push(format!("FOMOD source not found: {}", ft.source));
            }
            return Ok(());
        }
    };

    // Normalize destination: FOMOD XML uses Windows backslashes and may include a
    // leading ".\" (e.g. ".\DIP") which becomes "./" after normalization and must be stripped.
    // - destination=None  → unwrap_or(source) gives the source path (non-empty) → subfolder
    // - destination=""    → dest_base is empty → install to Data root
    // - destination="./x" → strip "./" → "x"
    let dest_base = ft.destination.unwrap_or(ft.source).replace('\\', "/");
    let dest_base = dest_base.trim_end_matches('/').trim_start_matches("./");

    let src_is_dir = source_path.is_dir();
    let src_is_file = source_path.is_file();
    dlog!(
        "[deployd] FOMOD collect: source={:?} is_dir={src_is_dir} is_file={src_is_file}",
        source_path.display()
    );

    if src_is_dir {
        let mut file_count = 0usize;
        for entry in WalkDir::new(&source_path) {
            let entry = entry?;
            // Never emit the source directory itself as a file entry — WalkDir always yields
            // the root as its first element; guard against any edge case where the root's
            // file_type() could be misreported.
            if entry.path() == source_path {
                continue;
            }
            if entry.file_type().is_file() {
                let rel_to_source = entry.path().strip_prefix(&source_path)?;
                mappings.push(FomodFileMapping {
                    source_relative: entry.path().strip_prefix(extracted_root)?.to_path_buf(),
                    dest_relative: Path::new(dest_base).join(rel_to_source),
                });
                file_count += 1;
            }
        }
        if file_count == 0 {
            dlog!(
                "[deployd] FOMOD: directory source has no files: {}",
                source_path.display()
            );
            // Emit a directory sentinel so the installer still creates the folder in the
            // game directory. The installer identifies sentinels by source_relative pointing
            // to a directory (src_abs.is_dir() check in add_mod_with_file_list).
            mappings.push(FomodFileMapping {
                source_relative: source_path.strip_prefix(extracted_root)?.to_path_buf(),
                dest_relative: PathBuf::from(dest_base),
            });
        }
    } else if src_is_file {
        let dest = if dest_base.is_empty() {
            // No destination — use normalized source path
            PathBuf::from(ft.source.replace('\\', "/"))
        } else if dest_base.ends_with('/') {
            Path::new(dest_base).join(source_path.file_name().unwrap_or_default())
        } else {
            PathBuf::from(dest_base)
        };
        mappings.push(FomodFileMapping {
            source_relative: source_path.strip_prefix(extracted_root)?.to_path_buf(),
            dest_relative: dest,
        });
    } else {
        dlog!(
            "[deployd] FOMOD: source is neither file nor dir (broken symlink or special file?): {}",
            source_path.display()
        );
    }

    Ok(())
}
