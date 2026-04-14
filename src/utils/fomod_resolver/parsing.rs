use std::path::Path;

use anyhow::Result;

use super::io::read_fomod_xml;
use super::types::{
    FomodGroupType, FomodUiConfig, FomodUiDependencies, FomodUiGroup, FomodUiPlugin, FomodUiStep,
};
use super::xml_structs::{XmlConfig, XmlDependencies};

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
                                                    dep_type_default: p
                                                        .type_descriptor
                                                        .as_ref()
                                                        .and_then(|td| td.dependency_type.as_ref())
                                                        .and_then(|dt| dt.default_type.as_ref())
                                                        .map(|t| t.name.clone())
                                                        .unwrap_or_default(),
                                                    dep_type_patterns: p
                                                        .type_descriptor
                                                        .as_ref()
                                                        .and_then(|td| td.dependency_type.as_ref())
                                                        .and_then(|dt| dt.patterns.as_ref())
                                                        .map(|ps| {
                                                            ps.pattern
                                                                .iter()
                                                                .filter_map(|pat| {
                                                                    let type_name = pat
                                                                        .typ
                                                                        .as_ref()?
                                                                        .name
                                                                        .clone();
                                                                    let deps = pat
                                                                        .dependencies
                                                                        .as_ref()
                                                                        .map(convert_deps_to_ui)
                                                                        .unwrap_or_else(|| {
                                                                            FomodUiDependencies {
                                                                                operator: "And"
                                                                                    .to_string(),
                                                                                flag_deps: vec![],
                                                                                file_deps: vec![],
                                                                                nested: vec![],
                                                                            }
                                                                        });
                                                                    Some((deps, type_name))
                                                                })
                                                                .collect()
                                                        })
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
                                                    image_path: p
                                                        .image
                                                        .as_ref()
                                                        .map(|img| img.path.clone()),
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

pub(super) fn convert_deps_to_ui(deps: &XmlDependencies) -> FomodUiDependencies {
    FomodUiDependencies {
        operator: deps.operator.clone(),
        flag_deps: deps
            .flag_dependencies
            .iter()
            .map(|f| (f.flag.clone(), f.value.clone()))
            .collect(),
        file_deps: deps
            .file_dependencies
            .iter()
            .map(|f| (f.file.to_lowercase().replace('\\', "/"), f.state.clone()))
            .collect(),
        nested: deps.nested.iter().map(convert_deps_to_ui).collect(),
    }
}

pub(super) fn parse_group_type(s: &str) -> FomodGroupType {
    match s {
        "SelectAll" => FomodGroupType::SelectAll,
        "SelectExactlyOne" => FomodGroupType::SelectExactlyOne,
        "SelectAtLeastOne" => FomodGroupType::SelectAtLeastOne,
        "SelectAtMostOne" => FomodGroupType::SelectAtMostOne,
        "SelectAny" => FomodGroupType::SelectAny,
        _ => FomodGroupType::SelectAny,
    }
}
