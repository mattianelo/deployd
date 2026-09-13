use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};

use super::Alternate;

fn groups(alternates: &[Alternate]) -> BTreeMap<&str, Vec<&Alternate>> {
    let mut groups: BTreeMap<&str, Vec<&Alternate>> = BTreeMap::new();
    for alternate in alternates {
        if let Some(group) = &alternate.group {
            groups.entry(group).or_default().push(alternate);
        }
    }
    groups
}

pub(super) fn validate(alternates: &[Alternate]) -> Result<()> {
    for (name, options) in groups(alternates) {
        ensure!(
            options.iter().filter(|option| option.default).count() == 1,
            "Option group '{name}' must have exactly one default"
        );
    }
    Ok(())
}

pub(super) fn validate_choices(
    alternates: &[Alternate],
    selected: &BTreeSet<String>,
) -> Result<()> {
    for (name, options) in groups(alternates) {
        ensure!(
            options
                .iter()
                .filter(|option| selected.contains(&option.key))
                .count()
                == 1,
            "Select exactly one option from '{name}'"
        );
    }
    Ok(())
}
