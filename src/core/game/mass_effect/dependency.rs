use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

use super::manifest::validate_dlc;

pub(super) struct Dependency<'a> {
    name: &'a str,
    present: bool,
    minimum: Option<[i32; 4]>,
    maximum: Option<[i32; 4]>,
    options: Vec<(Option<bool>, String)>,
}

impl<'a> Dependency<'a> {
    pub(super) fn parse(value: &'a str, format: &str) -> Result<Self> {
        let parsed = Self::parse_condition(value, format)?;
        ensure!(
            parsed.maximum.is_none(),
            "Maximum versions are only supported in conditional DLC setups"
        );
        Ok(parsed)
    }

    pub(super) fn parse_condition(value: &'a str, format: &str) -> Result<Self> {
        let (present, value) = if let Some(value) = value.strip_prefix('-') {
            (false, value)
        } else {
            (true, value.strip_prefix('+').unwrap_or(value))
        };
        let (name, constraints) = value
            .find(['[', '('])
            .map(|index| value.split_at(index))
            .unwrap_or((value, ""));
        validate_dlc(name)?;
        ensure!(!name.contains([']', ')']), "Malformed DLC requirement");
        let mut result = Self {
            name,
            present,
            minimum: None,
            maximum: None,
            options: Vec::new(),
        };
        if !constraints.is_empty() {
            ensure!(
                format.starts_with('8') || format.starts_with('9'),
                "DLC version requirements need moddesc 8 or newer"
            );
            let close = if constraints.starts_with('[') {
                ']'
            } else {
                ')'
            };
            ensure!(
                format.starts_with('8') || close == ']',
                "Keyed DLC requirements must use square brackets"
            );
            let body = constraints[1..]
                .strip_suffix(close)
                .context("Malformed DLC requirement")?;
            if format.starts_with('8') {
                result.minimum = Some(numeric_version(body.trim())?);
            } else {
                for entry in super::alternates::syntax::split(body, ',')? {
                    let (key, value) = entry
                        .split_once('=')
                        .context("Expected keyed DLC requirement")?;
                    match key.trim().to_ascii_lowercase().as_str() {
                        "minversion" => {
                            ensure!(result.minimum.is_none(), "Duplicate minimum version");
                            result.minimum = Some(numeric_version(value.trim())?);
                        }
                        "maxversion" => {
                            ensure!(result.maximum.is_none(), "Duplicate maximum version");
                            result.maximum = Some(numeric_version(value.trim())?);
                        }
                        "optionkey" => {
                            let body = value
                                .trim()
                                .strip_prefix('[')
                                .and_then(|value| value.strip_suffix(']'))
                                .context("DLC optionkey must be a bracketed object")?;
                            let mut option = None;
                            let mut label = false;
                            for field in super::alternates::syntax::split(body, ',')? {
                                let (key, value) =
                                    field.split_once('=').context("Malformed DLC option key")?;
                                let value = value.trim().trim_matches('"');
                                match key.trim().to_ascii_lowercase().as_str() {
                                    "option" => {
                                        ensure!(
                                            option.is_none() && !value.is_empty(),
                                            "Invalid or duplicate DLC option"
                                        );
                                        option = Some(value);
                                    }
                                    "uistring" => {
                                        ensure!(!label, "Duplicate DLC option label");
                                        label = true;
                                    }
                                    _ => anyhow::bail!("Unknown DLC option descriptor '{key}'"),
                                }
                            }
                            let value = option.context("DLC optionkey needs option")?;
                            let (required, value) = if let Some(value) = value.strip_prefix('+') {
                                (Some(true), value)
                            } else if let Some(value) = value.strip_prefix('-') {
                                (Some(false), value)
                            } else {
                                (None, value)
                            };
                            ensure!(!value.is_empty(), "Empty DLC option reference");
                            result.options.push((required, value.to_lowercase()));
                        }
                        _ => anyhow::bail!("Unsupported DLC requirement condition '{key}'"),
                    }
                }
            }
        }
        Ok(result)
    }

    pub(super) fn present_in(&self, available: &BTreeSet<String>) -> bool {
        available.contains(&self.name.to_lowercase())
    }

    #[cfg(test)]
    pub(super) fn matches(
        &self,
        available: &BTreeSet<String>,
        versions: &BTreeMap<String, String>,
    ) -> Result<bool> {
        self.matches_options(available, versions, &BTreeMap::new())
    }

    pub(super) fn matches_options(
        &self,
        available: &BTreeSet<String>,
        versions: &BTreeMap<String, String>,
        options: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<bool> {
        let name = self.name.to_lowercase();
        if !self.present {
            return Ok(!available.contains(&name));
        }
        if !available.contains(&name) {
            return Ok(false);
        }
        if self.minimum.is_some() || self.maximum.is_some() {
            let value = versions.get(&name).with_context(|| format!("Cannot verify the version of DLC '{}'; include its source package in the enabled recipe", self.name))?;
            let version = numeric_version(value)?;
            if self.minimum.is_some_and(|minimum| version < minimum)
                || self.maximum.is_some_and(|maximum| version > maximum)
            {
                return Ok(false);
            }
        }
        if !self.options.is_empty() {
            let selected = options.get(&name).with_context(|| {
                format!("Cannot verify the installed options of DLC '{}'", self.name)
            })?;
            if self.options.iter().any(|(required, key)| {
                required.is_some_and(|required| selected.contains(key) != required)
            }) {
                return Ok(false);
            }
            if self.options.iter().any(|(required, _)| required.is_none())
                && !self
                    .options
                    .iter()
                    .any(|(required, key)| required.is_none() && selected.contains(key))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn numeric_version(value: &str) -> Result<[i32; 4]> {
    let parts: Vec<_> = value.split('.').collect();
    ensure!(
        (2..=4).contains(&parts.len()),
        "Invalid numeric mod version '{value}'"
    );
    let mut version = [0; 4];
    for (index, part) in parts.iter().enumerate() {
        ensure!(
            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()),
            "Invalid numeric mod version '{value}'"
        );
        version[index] = part
            .parse()
            .with_context(|| format!("Invalid numeric mod version '{value}'"))?;
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn minimum_versions_compare_numeric_components_and_require_known_provenance() -> Result<()> {
        let requirement = Dependency::parse("DLC_MOD_CP[minversion=1.9.0.1]", "9.1")?;
        let available = ["dlc_mod_cp".into()].into();
        for (version, matches) in [
            ("2.0", true),
            ("1.10", true),
            ("1.9.0.1", true),
            ("1.9", false),
            ("1.8.99", false),
        ] {
            assert_eq!(
                requirement.matches(&available, &[("dlc_mod_cp".into(), version.into())].into())?,
                matches
            );
        }
        assert!(!requirement.matches(&BTreeSet::new(), &BTreeMap::new())?);
        assert!(requirement.matches(&available, &BTreeMap::new()).is_err());
        assert_eq!(numeric_version("2.0")?, numeric_version("2.0.0.0")?);
        for invalid in ["2", "2.beta", "2.-1", "2.0.0.0.1", "2.2147483648", "2..0"] {
            assert!(numeric_version(invalid).is_err());
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn version_syntax_is_gated_and_unknown_conditions_fail_closed() -> Result<()> {
        for (format, suffix) in [
            ("9.1", "[MINVERSION=1.9.0.1]"),
            ("8.2", "[1.9]"),
            ("8.0", "(1.9)"),
        ] {
            Dependency::parse(&format!("DLC_MOD_CP{suffix}"), format)?;
        }
        for (format, suffix) in [
            ("7", "[1.9]"),
            ("8.2", "[minversion=1.9]"),
            ("9.1", "(1.9)"),
            ("9.1", "[1.9]"),
            ("9.1", "[minversion=1.9]junk"),
            ("9.1", "[minversion=1.9,optionkey=A]"),
            ("9.1", "[maxversion=1.9]"),
            ("9.1", "]"),
        ] {
            assert!(
                Dependency::parse(&format!("DLC_MOD_CP{suffix}"), format).is_err(),
                "accepted {format}: {suffix}"
            );
        }
        Ok(())
    }
}
