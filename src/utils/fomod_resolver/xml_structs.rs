use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct XmlConfig {
    #[serde(rename = "requiredInstallFiles")]
    pub(super) required_install_files: Option<XmlFileList>,

    #[serde(rename = "installSteps")]
    pub(super) install_steps: Option<XmlStepList>,

    #[serde(rename = "conditionalFileInstalls")]
    pub(super) conditional_file_installs: Option<XmlConditionalInstalls>,
    // moduleName, moduleImage, moduleDependencies → silently ignored.
}

#[derive(Debug, Deserialize)]
pub(super) enum XmlFileEntry {
    #[serde(rename = "file")]
    File(XmlFileAttrs),
    #[serde(rename = "folder")]
    Folder(XmlFileAttrs),
}

impl XmlFileEntry {
    pub(super) fn attrs(&self) -> super::path_index::FileRef<'_> {
        match self {
            XmlFileEntry::File(a) | XmlFileEntry::Folder(a) => super::path_index::FileRef {
                source: &a.source,
                destination: a.destination.as_deref(),
            },
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct XmlFileList {
    #[serde(rename = "$value", default)]
    pub(super) items: Vec<XmlFileEntry>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlFileAttrs {
    #[serde(rename = "@source")]
    pub(super) source: String,
    #[serde(rename = "@destination")]
    pub(super) destination: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlStepList {
    #[serde(rename = "installStep", default)]
    pub(super) install_step: Vec<XmlInstallStep>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlInstallStep {
    #[serde(rename = "@name", default)]
    pub(super) name: String,

    #[serde(rename = "optionalFileGroups")]
    pub(super) optional_file_groups: Option<XmlGroupList>,

    /// Visibility conditions — step is only shown when these are satisfied.
    pub(super) visible: Option<XmlDependencies>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlGroupList {
    #[serde(rename = "group", default)]
    pub(super) group: Vec<XmlGroup>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlGroup {
    #[serde(rename = "@name", default)]
    pub(super) name: String,

    #[serde(rename = "@type", default)]
    pub(super) typ: String,

    pub(super) plugins: Option<XmlPluginList>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlPluginList {
    #[serde(rename = "plugin", default)]
    pub(super) plugin: Vec<XmlPlugin>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlPlugin {
    #[serde(rename = "@name", default)]
    pub(super) name: String,

    pub(super) description: Option<XmlDescription>,

    pub(super) files: Option<XmlFileList>,

    #[serde(rename = "typeDescriptor")]
    pub(super) type_descriptor: Option<XmlTypeDescriptor>,

    #[serde(rename = "conditionFlags")]
    pub(super) condition_flags: Option<XmlConditionFlags>,
    // image → silently ignored.
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlDescription {
    #[serde(rename = "$text", default)]
    pub(super) text: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlTypeDescriptor {
    #[serde(rename = "type")]
    pub(super) typ: Option<XmlPluginType>,
    // dependencyType → silently ignored.
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlPluginType {
    #[serde(rename = "@name", default)]
    pub(super) name: String,
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct XmlConditionFlags {
    #[serde(rename = "flag", default)]
    pub(super) flags: Vec<XmlFlag>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlFlag {
    #[serde(rename = "@name")]
    pub(super) name: String,
    #[serde(rename = "$text", default)]
    pub(super) value: String,
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct XmlConditionalInstalls {
    pub(super) patterns: Option<XmlConditionalPatterns>,
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct XmlConditionalPatterns {
    #[serde(rename = "pattern", default)]
    pub(super) pattern: Vec<XmlConditionalPattern>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlConditionalPattern {
    pub(super) dependencies: Option<XmlDependencies>,
    pub(super) files: Option<XmlFileList>,
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlDependencies {
    #[serde(rename = "@operator", default = "default_operator_and")]
    pub(super) operator: String,

    #[serde(rename = "flagDependency", default)]
    pub(super) flag_dependencies: Vec<XmlFlagDependency>,

    #[serde(rename = "fileDependency", default)]
    pub(super) file_dependencies: Vec<XmlFileDependency>,

    /// Nested composite dependencies (recursive AND/OR).
    #[serde(rename = "dependencies", default)]
    pub(super) nested: Vec<XmlDependencies>,
}

pub(super) fn default_operator_and() -> String {
    "And".to_string()
}

#[derive(Debug, Deserialize)]
pub(super) struct XmlFlagDependency {
    #[serde(rename = "@flag")]
    pub(super) flag: String,
    #[serde(rename = "@value")]
    pub(super) value: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub(super) struct XmlFileDependency {
    #[serde(rename = "@file")]
    pub(super) file: String,
    #[serde(rename = "@state")]
    pub(super) state: String,
}
