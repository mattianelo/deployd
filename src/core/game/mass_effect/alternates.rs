use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context as _, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Target;
use super::manifest::{relative_directory, relative_path, validate_dlc};
use super::package::{FileMapping, PackagePlan};

pub(super) mod choices;
mod groups;
pub(super) mod images;
mod multilist;
pub(super) mod syntax;

pub(super) struct Context<'a> {
    pub(super) available: &'a BTreeSet<String>,
    pub(super) sizes: &'a BTreeMap<String, Option<u64>>,
    pub(super) versions: Option<&'a BTreeMap<String, String>>,
    pub(super) options: Option<&'a BTreeMap<String, BTreeSet<String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Alternate {
    #[serde(default)]
    basegame: bool,
    #[serde(default)]
    requirements: Vec<String>,
    #[serde(default)]
    pub(crate) hidden: bool,
    #[serde(default)]
    pub(crate) sort_index: u32,
    #[serde(default)]
    pub(crate) image: Option<(String, u32)>,
    #[serde(default)]
    automatic_text: Option<String>,
    #[serde(default)]
    automatic_absent_text: Option<String>,
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) default: bool,
    #[serde(default)]
    pub(crate) group: Option<String>,
    #[serde(default)]
    option_key: Option<String>,
    #[serde(default)]
    dependency: Option<choices::Dependency>,
    condition: Condition,
    dlc: Vec<String>,
    operation: Operation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Condition {
    Manual,
    Always,
    AnyPresent,
    AnyAbsent,
    AllPresent,
    AllAbsent,
    Setup,
    SizedFiles(BTreeMap<String, u64>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Operation {
    File {
        source: Option<String>,
        destination: String,
        install: bool,
    },
    Folder {
        source: String,
        destination: String,
        new_dlc: bool,
    },
    Multilist(multilist::Multilist),
    Tlk(String),
    Merges(Vec<String>),
    Nothing,
}

impl Alternate {
    fn contextual(&self) -> bool {
        (!self.manual() && self.condition != Condition::Always) || !self.requirements.is_empty()
    }

    fn applicable(&self, context: &Context<'_>, format: &str) -> Result<bool> {
        for requirement in &self.requirements {
            if !super::dependency::Dependency::parse(requirement, format)?.matches_options(
                context.available,
                context.versions.unwrap_or(&BTreeMap::new()),
                context.options.unwrap_or(&BTreeMap::new()),
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn initial(
        &self,
        selected: &BTreeSet<String>,
        context: &Context<'_>,
        format: &str,
    ) -> Result<bool> {
        match &self.condition {
            Condition::Manual => Ok(selected.contains(&self.key)),
            Condition::Always => Ok(true),
            Condition::SizedFiles(files) => {
                for (path, size) in files {
                    match context.sizes.get(path) {
                        Some(Some(actual)) if actual == size => {}
                        Some(None) => bail!(
                            "Preparing deployment must transform '{path}' before evaluating file-size conditions"
                        ),
                        _ => return Ok(false),
                    }
                }
                Ok(true)
            }
            _ => {
                let requirements = self
                    .dlc
                    .iter()
                    .map(|value| super::dependency::Dependency::parse_condition(value, format))
                    .collect::<Result<Vec<_>>>()?;
                let presence = requirements
                    .iter()
                    .map(|requirement| requirement.present_in(context.available))
                    .collect::<Vec<_>>();
                let present = match self.condition {
                    Condition::AnyPresent => presence.iter().any(|value| *value),
                    Condition::AnyAbsent => return Ok(presence.iter().any(|value| !value)),
                    Condition::AllAbsent => return Ok(presence.iter().all(|value| !value)),
                    Condition::Setup => true,
                    _ => presence.iter().all(|value| *value),
                };
                if !present {
                    return Ok(false);
                }
                for (requirement, present) in requirements.iter().zip(presence) {
                    if (present || self.condition == Condition::Setup)
                        && !requirement.matches_options(
                            context.available,
                            context.versions.unwrap_or(&BTreeMap::new()),
                            context.options.unwrap_or(&BTreeMap::new()),
                        )?
                    {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }

    fn reference_key(&self) -> String {
        if let Some(key) = &self.option_key {
            return key.clone();
        }
        let name = format!("{}{}", self.name, self.group.as_deref().unwrap_or(""));
        let bytes: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        crc32fast::hash(&bytes)
            .to_le_bytes()
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect()
    }

    pub(super) fn merge_files(&self) -> &[String] {
        match &self.operation {
            Operation::Merges(names) => names,
            _ => &[],
        }
    }

    fn destination(&self, path: &str) -> Result<String> {
        if self.basegame {
            super::package::basegame_destination(path)
        } else {
            Ok(format!("DLC/{path}"))
        }
    }

    pub(super) fn required_files(&self) -> Vec<&str> {
        match &self.condition {
            Condition::SizedFiles(files) => files.keys().map(String::as_str).collect(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn tlk_key(&self) -> Option<&str> {
        if let Operation::Tlk(key) = &self.operation {
            Some(key)
        } else {
            None
        }
    }

    pub(crate) fn manual(&self) -> bool {
        self.condition == Condition::Manual
    }

    pub(super) fn parse(value: Option<&str>, folders: bool, target: Target) -> Result<Vec<Self>> {
        Self::parse_in(value, folders, target, false)
    }

    pub(super) fn parse_in(
        value: Option<&str>,
        folders: bool,
        target: Target,
        basegame: bool,
    ) -> Result<Vec<Self>> {
        let mut result = Vec::new();
        for mut fields in
            syntax::parse(value.filter(|text| !text.trim().is_empty()).unwrap_or("()"))?
        {
            let name = take(&mut fields, "friendlyname")?;
            let description = fields.remove("description").unwrap_or_default();
            let condition = match take(&mut fields, "condition")?.as_str() {
                "COND_SPECIFIC_SIZED_FILES" if folders => {
                    let paths = take(&mut fields, "requiredfilerelativepaths")?;
                    let sizes = take(&mut fields, "requiredfilesizes")?;
                    let paths = paths.split(';').collect::<Vec<_>>();
                    let sizes = sizes.split(';').collect::<Vec<_>>();
                    ensure!(
                        !paths.is_empty() && paths.len() == sizes.len() && paths.len() <= 1024,
                        "File-size conditions require matching nonempty path and size lists"
                    );
                    let mut required = BTreeMap::new();
                    for (path, size) in paths.into_iter().zip(sizes) {
                        let path = relative_path(path.trim())?;
                        let size = size
                            .trim()
                            .parse::<u64>()
                            .context("Invalid conditional file size")?;
                        ensure!(
                            size <= i64::MAX as u64
                                && required.insert(path.to_lowercase(), size).is_none(),
                            "Invalid or duplicate conditional file size"
                        );
                    }
                    Condition::SizedFiles(required)
                }
                "COND_SPECIFIC_DLC_SETUP" if folders => Condition::Setup,
                "COND_MANUAL" => Condition::Manual,
                "COND_ALWAYS" if !folders => Condition::Always,
                "COND_DLC_PRESENT" => Condition::AnyPresent,
                "COND_DLC_NOT_PRESENT" if folders => Condition::AnyAbsent,
                "COND_DLC_NOT_PRESENT" => Condition::AllAbsent,
                "COND_ANY_DLC_PRESENT" if folders => Condition::AnyPresent,
                "COND_ANY_DLC_NOT_PRESENT" if folders => Condition::AnyAbsent,
                "COND_ALL_DLC_PRESENT" if folders => Condition::AllPresent,
                "COND_ALL_DLC_NOT_PRESENT" if folders => Condition::AllAbsent,
                value => bail!("Unsupported alternate condition '{value}'"),
            };
            let dlc = fields
                .remove("conditionaldlc")
                .map(|value| {
                    value
                        .split(';')
                        .map(|name| {
                            let name = name.trim();
                            validate_dlc(
                                name.split('[')
                                    .next()
                                    .unwrap_or(name)
                                    .trim_start_matches(['+', '-']),
                            )?;
                            Ok(name.to_owned())
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            ensure!(dlc.len() <= 1024, "Too many alternate DLC conditions");
            ensure!(
                dlc.is_empty()
                    == matches!(
                        condition,
                        Condition::Manual | Condition::Always | Condition::SizedFiles(_)
                    ),
                "Alternate '{name}' has missing or inapplicable ConditionalDLC"
            );
            let default = fields
                .remove("checkedbydefault")
                .map(|value| match value.to_ascii_lowercase().as_str() {
                    "true" => Ok(true),
                    "false" => Ok(false),
                    _ => bail!("Invalid alternate CheckedByDefault"),
                })
                .transpose()?
                .unwrap_or(false);
            ensure!(
                !default || condition == Condition::Manual,
                "Only manual alternates can be checked by default"
            );
            let group = fields.remove("optiongroup");
            if let Some(group) = &group {
                ensure!(
                    condition == Condition::Manual
                        && !group.trim().is_empty()
                        && !group.contains(';'),
                    "OptionGroup requires a named manual choice"
                );
            }
            let option_key = fields.remove("optionkey");
            if let Some(key) = &option_key {
                ensure!(
                    !key.is_empty()
                        && key.len() <= 256
                        && !key.contains(';')
                        && key.trim() == key
                        && !key.chars().any(char::is_control),
                    "Invalid OptionKey"
                );
            }
            let requirements = fields
                .remove("dlcrequirements")
                .map(|value| {
                    value
                        .split(';')
                        .map(|item| item.trim().to_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            ensure!(
                requirements.is_empty() || condition == Condition::Manual,
                "DLCRequirements only applies to manual choices"
            );
            let hidden = fields
                .remove("hidden")
                .map(|value| value.parse::<bool>().context("Invalid Hidden boolean"))
                .transpose()?
                .unwrap_or(false);
            ensure!(
                !hidden || group.is_none(),
                "Grouped options cannot be hidden"
            );
            let sort_index = fields
                .remove("sortindex")
                .map(|value| value.parse::<u32>().context("Invalid SortIndex"))
                .transpose()?;
            ensure!(
                sort_index.is_none_or(|value| value > 0 && value <= i32::MAX as u32),
                "SortIndex must be a positive integer"
            );
            let image = fields
                .remove("imageassetname")
                .map(|value| -> Result<_> {
                    let name = relative_path(&value)?;
                    let height = take(&mut fields, "imageheight")?
                        .parse::<u32>()
                        .context("Invalid ImageHeight")?;
                    ensure!(height <= 1040, "ImageHeight exceeds supported size");
                    Ok((name, height))
                })
                .transpose()?;
            let automatic_text = fields.remove("applicableautotext");
            let automatic_absent_text = fields.remove("notapplicableautotext");
            let dependency = choices::parse(&mut fields)?;
            let operation = match take(&mut fields, "modoperation")?.as_str() {
                "OP_NOTHING" => Operation::Nothing,
                "OP_APPLY_MERGEMODS" if basegame && !folders => {
                    let names = take(&mut fields, "mergefiles")?
                        .split(';')
                        .map(|name| relative_path(name.trim()))
                        .collect::<Result<Vec<_>>>()?;
                    ensure!(
                        names.iter().all(|name| !name.contains('/')
                            && name.to_ascii_lowercase().ends_with(".m3m")),
                        "MergeFiles must name M3M files in MergeMods"
                    );
                    Operation::Merges(names)
                }
                "OP_APPLY_MULTILISTFILES" if basegame && !folders => {
                    Operation::Multilist(multilist::Multilist::parse_in(&mut fields, false, true)?)
                }
                "OP_ENABLE_TLKMERGE_OPTIONKEY" if folders && target == Target::Le1 => {
                    Operation::Tlk(take(&mut fields, "le1tlkoptionkey")?)
                }
                "OP_ADD_MULTILISTFILES_TO_CUSTOMDLC" if folders => {
                    Operation::Multilist(multilist::Multilist::parse(&mut fields, false)?)
                }
                "OP_NOINSTALL_MULTILISTFILES" if !folders => Operation::Multilist(
                    multilist::Multilist::parse_in(&mut fields, true, basegame)?,
                ),
                op @ ("OP_INSTALL" | "OP_SUBSTITUTE" | "OP_NOINSTALL") if !folders => {
                    let destination = relative_path(&take(&mut fields, "modfile")?)?;
                    let source = if op == "OP_NOINSTALL" {
                        None
                    } else {
                        let value = fields
                            .remove("altfile")
                            .or_else(|| fields.remove("modaltfile"))
                            .context("Alternate file needs AltFile")?;
                        Some(relative_path(&value)?)
                    };
                    Operation::File {
                        source,
                        destination,
                        install: op == "OP_INSTALL",
                    }
                }
                op @ ("OP_ADD_CUSTOMDLC" | "OP_ADD_FOLDERFILES_TO_CUSTOMDLC") if folders => {
                    let source = relative_directory(&take(&mut fields, "modaltdlc")?)?;
                    let destination = relative_directory(&take(&mut fields, "moddestdlc")?)?;
                    if op == "OP_ADD_CUSTOMDLC" {
                        validate_dlc(&destination)?;
                    }
                    Operation::Folder {
                        source,
                        destination,
                        new_dlc: op == "OP_ADD_CUSTOMDLC",
                    }
                }
                value => bail!("Unsupported alternate operation '{value}'"),
            };
            if let Operation::File { destination, .. } | Operation::Folder { destination, .. } =
                &operation
                && !basegame
            {
                let dlc = destination
                    .split('/')
                    .next()
                    .context("Missing alternate DLC destination")?;
                validate_dlc(dlc)?;
                ensure!(
                    !target.is_official_dlc(dlc),
                    "CUSTOMDLC alternates cannot modify official DLC '{dlc}'"
                );
            }
            ensure!(
                fields.is_empty(),
                "Unsupported alternate fields: {}",
                fields.keys().cloned().collect::<Vec<_>>().join(", ")
            );
            let key = format!(
                "alternate:{}:{}",
                if basegame {
                    "basegame"
                } else if folders {
                    "dlc"
                } else {
                    "file"
                },
                format_args!("{:x}", Sha256::digest(name.as_bytes()))
            );
            ensure!(
                !result.iter().any(|alt: &Self| alt.key == key),
                "Duplicate alternate FriendlyName"
            );
            result.push(Self {
                basegame,
                requirements,
                hidden,
                sort_index: sort_index.unwrap_or_default(),
                image,
                automatic_text,
                automatic_absent_text,
                key,
                name,
                description,
                default,
                group,
                condition,
                option_key,
                dependency,
                dlc,
                operation,
            });
        }
        Ok(result)
    }

    #[cfg(test)]
    fn active_with_sizes(
        &self,
        selected: &BTreeSet<String>,
        available: &BTreeSet<String>,
        sizes: &BTreeMap<String, Option<u64>>,
    ) -> Result<bool> {
        self.initial(
            selected,
            &Context {
                available,
                sizes,
                versions: None,
                options: None,
            },
            "9.2",
        )
    }

    #[cfg(test)]
    pub(super) fn active(&self, selected: &BTreeSet<String>, available: &BTreeSet<String>) -> bool {
        self.active_with_sizes(selected, available, &BTreeMap::new())
            .unwrap_or(false)
    }
}

fn take(fields: &mut BTreeMap<String, String>, key: &str) -> Result<String> {
    fields
        .remove(key)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("Alternate needs {key}"))
}

fn prefix(plan: &PackagePlan) -> Result<String> {
    let manifest = plan
        .sources
        .iter()
        .find(|source| {
            source
                .relative
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("moddesc.ini"))
        })
        .context("Alternate package has no manifest")?;
    Ok(manifest
        .relative
        .rsplit_once('/')
        .map(|(parent, _)| format!("{parent}/"))
        .unwrap_or_default())
}

fn declared_dlc(plan: &PackagePlan, destination: &str) -> bool {
    let Some(name) = destination.split('/').next() else {
        return false;
    };
    plan.manifest
        .dlc
        .iter()
        .any(|(_, dlc)| name.eq_ignore_ascii_case(dlc))
        || plan.manifest.alternates.iter().any(|alternate| {
            matches!(
                &alternate.operation,
                Operation::Folder {
                    destination,
                    new_dlc: true,
                    ..
                } if name.eq_ignore_ascii_case(destination)
            )
        })
}

fn selected_dlc(plan: &PackagePlan, selected: &BTreeSet<String>, destination: &str) -> bool {
    let Some(name) = destination.split('/').next() else {
        return false;
    };
    plan.manifest
        .dlc
        .iter()
        .any(|(_, dlc)| name.eq_ignore_ascii_case(dlc))
        || plan.manifest.alternates.iter().any(|alternate| {
            selected.contains(&alternate.key)
                && matches!(
                    &alternate.operation,
                    Operation::Folder {
                        destination,
                        new_dlc: true,
                        ..
                    } if name.eq_ignore_ascii_case(destination)
                )
        })
}

pub(super) fn mappings(plan: &PackagePlan, alternate: &Alternate) -> Result<Vec<FileMapping>> {
    let prefix = prefix(plan)?;
    let mut files = Vec::new();
    match &alternate.operation {
        Operation::Nothing => {}
        Operation::Merges(names) => {
            for name in names {
                ensure!(
                    plan.m3m.iter().any(|merge| merge
                        .source
                        .relative
                        .eq_ignore_ascii_case(&format!("{prefix}MergeMods/{name}"))),
                    "Missing alternate merge '{name}'"
                );
            }
        }
        Operation::Tlk(key) => {
            ensure!(
                plan.manifest.format.starts_with('9'),
                "Conditional TLK option activation requires moddesc 9 or newer"
            );
            ensure!(
                plan.embedded_tlk
                    .as_ref()
                    .is_some_and(|tlk| tlk.option_keys.contains(key)),
                "Alternate references an unknown TLK option '{key}'"
            );
        }
        Operation::Multilist(list) => files.extend(list.mappings(plan)?),
        Operation::File {
            source,
            destination,
            install,
        } => {
            let target = alternate.destination(destination)?;
            ensure!(
                alternate.basegame || declared_dlc(plan, destination),
                "Alternate file must target a declared custom DLC"
            );
            ensure!(
                *install
                    || plan
                        .files
                        .iter()
                        .any(|file| file.destination.eq_ignore_ascii_case(&target)),
                "Alternate target '{destination}' is not installed by this package"
            );
            if let Some(source) = source {
                let name = format!("{prefix}{source}");
                let file = plan
                    .sources
                    .iter()
                    .find(|file| file.relative.eq_ignore_ascii_case(&name))
                    .with_context(|| format!("Alternate source '{source}' is missing"))?;
                files.push(FileMapping {
                    source: file.relative.clone(),
                    destination: target,
                });
            }
        }
        Operation::Folder {
            source,
            destination,
            new_dlc,
        } => {
            if !new_dlc {
                ensure!(
                    declared_dlc(plan, destination),
                    "Alternate folder must target a declared custom DLC"
                );
            }
            let prefix = if source == "." {
                prefix
            } else {
                format!("{prefix}{source}/")
            };
            for file in &plan.sources {
                if file
                    .relative
                    .to_lowercase()
                    .starts_with(&prefix.to_lowercase())
                {
                    files.push(FileMapping {
                        source: file.relative.clone(),
                        destination: format!(
                            "DLC/{destination}/{}",
                            file.relative
                                .split('/')
                                .skip(prefix.matches('/').count())
                                .collect::<Vec<_>>()
                                .join("/")
                        ),
                    });
                }
            }
            ensure!(
                !files.is_empty(),
                "Alternate folder '{source}' is missing or empty"
            );
        }
    }
    for file in &files {
        super::binary::mapping(file)?;
        ensure!(
            super::package::transformation(&file.destination)
                .or_else(|| super::package::transformation(&file.source))
                .is_none_or(|kind| kind.supported()),
            "Alternate requires an unsupported transformation"
        );
        let destination = super::binary::game_path(&file.destination)?;
        super::package::validate_mapped_inert_payload(&destination)?;
    }
    Ok(files)
}

pub(super) fn validate(plan: &PackagePlan) -> Result<()> {
    groups::validate(&plan.manifest.alternates)?;
    choices::Model::new(&plan.manifest.alternates, &plan.manifest.format)?;
    for alternate in &plan.manifest.alternates {
        ensure!(
            !plan.manifest.format.starts_with('7')
                || (!alternate.hidden && alternate.sort_index == 0),
            "Hidden and SortIndex require moddesc 8 or newer"
        );
        for requirement in &alternate.requirements {
            super::dependency::Dependency::parse(requirement, &plan.manifest.format)?;
        }
        for requirement in &alternate.dlc {
            if alternate.condition == Condition::Setup {
                ensure!(
                    requirement.starts_with(['+', '-']),
                    "Specific DLC setup entries require + or -"
                );
            }
            super::dependency::Dependency::parse_condition(requirement, &plan.manifest.format)?;
        }
        mappings(plan, alternate)?;
    }
    Ok(())
}

pub(super) fn apply_with_context(
    plan: &mut PackagePlan,
    selected: &BTreeSet<String>,
    context: &Context<'_>,
) -> Result<()> {
    validate_choices(plan, selected)?;
    let effective = plan
        .choice_model()?
        .evaluate_context(selected, None, Some(context))?;
    apply_selected(plan, &effective.selected)
}

pub(super) fn apply_selected_choices(
    plan: &mut PackagePlan,
    selected: &BTreeSet<String>,
) -> Result<()> {
    validate_choices(plan, selected)?;
    let effective = plan.choice_model()?.evaluate(selected, None)?;
    apply_selected(plan, &effective.selected)
}

fn apply_selected(plan: &mut PackagePlan, selected: &BTreeSet<String>) -> Result<()> {
    plan.active_options = plan
        .manifest
        .alternates
        .iter()
        .filter(|alt| selected.contains(&alt.key))
        .map(|alt| alt.reference_key().to_lowercase())
        .collect();
    plan.active_tlk.clear();
    let original = plan.clone();
    let mut merge_names: BTreeSet<_> = plan
        .manifest
        .merges
        .iter()
        .map(|name| name.to_lowercase())
        .collect();
    let exclusions = |alternate: &&Alternate| matches!(&alternate.operation, Operation::Multilist(list) if list.removal());
    for alternate in original
        .manifest
        .alternates
        .iter()
        .filter(|alt| !exclusions(alt))
        .chain(original.manifest.alternates.iter().filter(exclusions))
    {
        if !selected.contains(&alternate.key) {
            continue;
        }
        if !alternate.basegame {
            match &alternate.operation {
                Operation::File { destination, .. }
                | Operation::Folder {
                    destination,
                    new_dlc: false,
                    ..
                } => ensure!(
                    selected_dlc(&original, selected, destination),
                    "Selected alternate must target an installed custom DLC"
                ),
                _ => {}
            }
        }
        match &alternate.operation {
            Operation::Merges(names) => {
                merge_names.extend(names.iter().map(|name| name.to_lowercase()));
            }
            Operation::Tlk(key) => {
                plan.active_tlk.insert(key.clone());
            }
            Operation::Multilist(list) if list.removal() => {
                let excluded = list.destinations(&original)?;
                plan.files.retain(|file| {
                    !excluded
                        .iter()
                        .any(|path| path.eq_ignore_ascii_case(&file.destination))
                });
            }
            Operation::File {
                source: None,
                destination,
                ..
            } => {
                let target = alternate.destination(destination)?;
                plan.files
                    .retain(|file| !file.destination.eq_ignore_ascii_case(&target));
            }
            _ => {
                for mut file in mappings(&original, alternate)? {
                    if let Some(existing) = plan.files.iter_mut().find(|existing| {
                        existing.destination.eq_ignore_ascii_case(&file.destination)
                    }) {
                        file.destination = existing.destination.clone();
                        *existing = file;
                    } else {
                        plan.files.push(file);
                    }
                }
            }
        }
        if let Operation::Folder {
            source,
            destination,
            new_dlc: true,
        } = &alternate.operation
            && !plan
                .manifest
                .dlc
                .iter()
                .any(|(_, dlc)| dlc.eq_ignore_ascii_case(destination))
        {
            plan.manifest
                .dlc
                .push((source.clone(), destination.clone()));
        }
    }
    plan.m3m.retain(|merge| {
        merge
            .source
            .relative
            .rsplit('/')
            .next()
            .is_some_and(|name| merge_names.contains(&name.to_lowercase()))
    });
    plan.jobs.retain(|job| {
        job.kind != super::package::Transformation::MergeMod
            || plan
                .m3m
                .iter()
                .any(|merge| merge.source.relative == job.source)
    });
    Ok(())
}

#[cfg(test)]
mod choices_tests;
#[cfg(test)]
mod tests;

pub(crate) fn validate_choices(plan: &PackagePlan, selected: &BTreeSet<String>) -> Result<()> {
    ensure!(
        selected.is_subset(&plan.option_keys()),
        "Unknown MELE installation choice"
    );
    groups::validate_choices(&plan.manifest.alternates, selected)
}
