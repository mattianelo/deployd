use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::{Alternate, Context as InstallContext, take};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Action {
    Allow,
    AllowChecked,
    Disallow,
    DisallowChecked,
}

impl Action {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "ACTION_ALLOW_SELECT" => Ok(Self::Allow),
            "ACTION_ALLOW_SELECT_CHECKED" => Ok(Self::AllowChecked),
            "ACTION_DISALLOW_SELECT" => Ok(Self::Disallow),
            "ACTION_DISALLOW_SELECT_CHECKED" => Ok(Self::DisallowChecked),
            _ => bail!("Unsupported option dependency action '{value}'"),
        }
    }
    fn apply(&self, selected: &mut bool, selectable: &mut bool, previous: bool) {
        let allowed = matches!(self, Self::Allow | Self::AllowChecked);
        if !allowed || !previous {
            *selected = matches!(self, Self::AllowChecked | Self::DisallowChecked);
        }
        *selectable = allowed;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Dependency {
    references: Vec<(String, bool)>,
    met: Action,
    not_met: Action,
}

pub(super) fn parse(fields: &mut BTreeMap<String, String>) -> Result<Option<Dependency>> {
    let Some(value) = fields.remove("dependsonkeys") else {
        return Ok(None);
    };
    let mut unique = BTreeSet::new();
    let references = value
        .split(';')
        .map(|item| {
            let item = item.trim();
            let (selected, name) = if let Some(name) = item.strip_prefix('+') {
                (true, name)
            } else if let Some(name) = item.strip_prefix('-') {
                (false, name)
            } else {
                bail!("DependsOnKeys entries must begin with + or -")
            };
            ensure!(
                !name.is_empty() && unique.insert(name.to_owned()),
                "Empty or duplicate option dependency key"
            );
            Ok((name.to_owned(), selected))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(references.len() <= 1024, "Too many option dependencies");
    Ok(Some(Dependency {
        references,
        met: Action::parse(&take(fields, "dependsonmetaction")?)?,
        not_met: Action::parse(&take(fields, "dependsonnotmetaction")?)?,
    }))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct State {
    pub(crate) selected: BTreeSet<String>,
    pub(crate) selectable: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Model {
    options: Vec<Alternate>,
    order: Vec<usize>,
    references: BTreeMap<String, usize>,
    legacy: bool,
    format: String,
    deferred: BTreeSet<usize>,
}

impl Model {
    pub(in crate::core::game::mass_effect) fn new(
        options: &[Alternate],
        format: &str,
    ) -> Result<Self> {
        let mut references = BTreeMap::new();
        for (index, option) in options.iter().enumerate() {
            if option.option_key.is_some() || option.dependency.is_some() {
                ensure!(
                    !format.starts_with('7'),
                    "Option dependency keys require moddesc 8 or newer"
                );
            }
            let key = option.reference_key();
            ensure!(
                references.insert(key.clone(), index).is_none() || format.starts_with('7'),
                "Duplicate OptionKey '{key}'"
            );
        }
        let mut order = Vec::new();
        let mut pending: BTreeSet<usize> = (0..options.len()).collect();
        for (index, option) in options.iter().enumerate() {
            if let Some(dependency) = &option.dependency {
                for (key, _) in &dependency.references {
                    let referenced = *references
                        .get(key)
                        .with_context(|| format!("Unknown option dependency '{key}'"))?;
                    ensure!(
                        referenced != index,
                        "Option dependencies cannot reference themselves"
                    );
                }
            }
        }
        while !pending.is_empty() {
            let ready = pending
                .iter()
                .copied()
                .find(|index| {
                    options[*index]
                        .dependency
                        .as_ref()
                        .is_none_or(|dependency| {
                            dependency.references.iter().all(|(key, _)| {
                                references
                                    .get(key)
                                    .is_some_and(|index| !pending.contains(index))
                            })
                        })
                })
                .context("Option dependencies contain a cycle")?;
            pending.remove(&ready);
            order.push(ready);
        }
        let mut deferred = BTreeSet::new();
        for index in &order {
            if options[*index].contextual()
                || options[*index]
                    .dependency
                    .as_ref()
                    .is_some_and(|dependency| {
                        dependency.references.iter().any(|(key, _)| {
                            references
                                .get(key)
                                .is_some_and(|index| deferred.contains(index))
                        })
                    })
            {
                deferred.insert(*index);
            }
        }
        Ok(Self {
            format: format.into(),
            deferred,
            options: options.to_vec(),
            order,
            references,
            legacy: format != "9.2",
        })
    }

    pub(crate) fn evaluate(
        &self,
        selected: &BTreeSet<String>,
        previous: Option<&State>,
    ) -> Result<State> {
        self.evaluate_context(selected, previous, None)
    }

    pub(super) fn evaluate_context(
        &self,
        selected: &BTreeSet<String>,
        previous: Option<&State>,
        context: Option<&InstallContext<'_>>,
    ) -> Result<State> {
        let mut state = State {
            selected: selected.clone(),
            selectable: self
                .options
                .iter()
                .filter(|option| option.manual())
                .map(|option| option.key.clone())
                .collect(),
        };
        for option in &self.options {
            if let Some(context) = context {
                if option.initial(selected, context, &self.format)? {
                    state.selected.insert(option.key.clone());
                } else {
                    state.selected.remove(&option.key);
                }
                if !option.applicable(context, &self.format)? {
                    state.selectable.remove(&option.key);
                    state.selected.remove(&option.key);
                }
            } else if matches!(option.condition, super::Condition::Always) {
                state.selected.insert(option.key.clone());
            }
        }
        for _ in 0..=self.options.len() {
            let before_iteration = state.clone();
            for index in &self.order {
                let option = &self.options[*index];
                if context.is_none() && self.deferred.contains(index) {
                    continue;
                }
                let Some(dependency) = &option.dependency else {
                    continue;
                };
                let mut active = state.selected.contains(&option.key);
                let mut selectable = state.selectable.contains(&option.key);
                let before = previous.is_none_or(|state| state.selectable.contains(&option.key));
                let mut met = true;
                for (key, required) in &dependency.references {
                    let referenced = self
                        .references
                        .get(key)
                        .context("Missing validated option dependency")?;
                    if state.selected.contains(&self.options[*referenced].key) != *required {
                        let current = if self.legacy { selectable } else { before };
                        dependency
                            .not_met
                            .apply(&mut active, &mut selectable, current);
                        met = false;
                        break;
                    }
                    if self.legacy {
                        let current = selectable;
                        dependency.met.apply(&mut active, &mut selectable, current);
                    }
                }
                if met && !self.legacy {
                    dependency.met.apply(&mut active, &mut selectable, before);
                }
                if active {
                    if let Some(group) = &option.group {
                        for peer in &self.options {
                            if peer.group.as_ref() == Some(group) && peer.key != option.key {
                                state.selected.remove(&peer.key);
                            }
                        }
                    }
                    state.selected.insert(option.key.clone());
                } else {
                    state.selected.remove(&option.key);
                }
                if selectable {
                    state.selectable.insert(option.key.clone());
                } else {
                    state.selectable.remove(&option.key);
                }
            }
            for option in &self.options {
                if let Some(group) = &option.group
                    && !self.options.iter().any(|peer| {
                        peer.group.as_ref() == Some(group) && state.selected.contains(&peer.key)
                    })
                {
                    let peers = self
                        .options
                        .iter()
                        .filter(|peer| {
                            peer.group.as_ref() == Some(group)
                                && state.selectable.contains(&peer.key)
                        })
                        .collect::<Vec<_>>();
                    if let Some(peer) = peers
                        .iter()
                        .find(|peer| peer.default)
                        .or_else(|| peers.first())
                    {
                        state.selected.insert(peer.key.clone());
                    }
                }
            }
            if state == before_iteration {
                return Ok(state);
            }
        }
        bail!("Installer choice constraints do not settle on a consistent selection")
    }
}
