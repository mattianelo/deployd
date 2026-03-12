use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::dlog;

use super::io::read_fomod_xml;
use super::path_index::{build_path_index, collect_file};
use super::types::{FomodFileMapping, FomodSelections};
use super::xml_structs::{XmlConfig, XmlDependencies, XmlGroup, XmlPlugin};

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
