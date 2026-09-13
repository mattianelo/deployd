use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::super::manifest::{join_directory, relative_directory, validate_dlc};
use super::super::package::{FileMapping, PackagePlan};
use super::{prefix, take};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Multilist {
    #[serde(default)]
    basegame: bool,
    id: i32,
    source: Option<String>,
    destination: String,
    flatten: bool,
}

impl Multilist {
    pub(super) fn parse(fields: &mut BTreeMap<String, String>, removal: bool) -> Result<Self> {
        Self::parse_in(fields, removal, false)
    }
    pub(super) fn parse_in(
        fields: &mut BTreeMap<String, String>,
        removal: bool,
        basegame: bool,
    ) -> Result<Self> {
        let id: i32 = take(fields, "multilistid")?
            .parse()
            .context("Invalid multilist ID")?;
        ensure!(id >= 0, "Invalid multilist ID");
        let source = if removal {
            None
        } else {
            Some(relative_directory(&take(fields, "multilistrootpath")?)?)
        };
        let destination = relative_directory(&take(
            fields,
            if removal || basegame {
                "multilisttargetpath"
            } else {
                "moddestdlc"
            },
        )?)?;
        let flatten = if removal {
            false
        } else {
            fields
                .remove("flattenmultilistoutput")
                .map(|value| match value.to_ascii_lowercase().as_str() {
                    "true" => Ok(true),
                    "false" => Ok(false),
                    _ => bail!("Invalid FlattenMultiListOutput"),
                })
                .transpose()?
                .unwrap_or(false)
        };
        Ok(Self {
            basegame,
            id,
            source,
            destination,
            flatten,
        })
    }

    pub(super) fn removal(&self) -> bool {
        self.source.is_none()
    }

    fn entries<'a>(&self, plan: &'a PackagePlan) -> Result<&'a Vec<String>> {
        ensure!(
            !self.flatten
                || !plan.manifest.format.starts_with('7')
                || plan.manifest.minimum_build >= 125,
            "Flattened multilists require moddesc 8 or moddesc 7 with minbuild 125"
        );
        (if self.basegame {
            &plan.manifest.base_multilists
        } else {
            &plan.manifest.multilists
        })
        .get(&self.id)
        .with_context(|| format!("Missing multilist{}", self.id))
    }

    pub(super) fn destinations(&self, plan: &PackagePlan) -> Result<Vec<String>> {
        if !self.basegame {
            let dlc = self
                .destination
                .split('/')
                .next()
                .context("Missing multilist DLC destination")?;
            validate_dlc(dlc)?;
            ensure!(
                !plan.manifest.target.is_official_dlc(dlc),
                "CUSTOMDLC multilists cannot modify official DLC"
            );
            ensure!(
                plan.manifest
                    .dlc
                    .iter()
                    .any(|(_, name)| name.eq_ignore_ascii_case(dlc)),
                "Multilist must target a declared custom DLC"
            );
        }
        let mut unique = BTreeSet::new();
        self.entries(plan)?
            .iter()
            .map(|entry| {
                let suffix = if self.flatten {
                    entry
                        .rsplit('/')
                        .next()
                        .context("Missing multilist filename")?
                } else {
                    entry
                };
                let destination = if self.basegame {
                    super::super::package::basegame_destination(&join_directory(
                        &self.destination,
                        suffix,
                    ))?
                } else {
                    format!("DLC/{}/{}", self.destination, suffix)
                };
                super::super::binary::game_path(&destination)?;
                ensure!(
                    unique.insert(destination.to_lowercase()),
                    "Multilist destinations collide after flattening"
                );
                Ok(destination)
            })
            .collect()
    }

    pub(super) fn mappings(&self, plan: &PackagePlan) -> Result<Vec<FileMapping>> {
        let destinations = self.destinations(plan)?;
        let Some(source) = &self.source else {
            return Ok(Vec::new());
        };
        let root = prefix(plan)?;
        self.entries(plan)?
            .iter()
            .zip(destinations)
            .map(|(entry, destination)| {
                let name = format!("{root}{}", join_directory(source, entry));
                let file = plan
                    .sources
                    .iter()
                    .find(|file| file.relative.eq_ignore_ascii_case(&name))
                    .with_context(|| format!("Multilist source '{name}' is missing"))?;
                Ok(FileMapping {
                    source: file.relative.clone(),
                    destination,
                })
            })
            .collect()
    }
}
