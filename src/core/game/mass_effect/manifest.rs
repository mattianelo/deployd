use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use super::Target;

pub(super) const FORMATS: &[&str] = &[
    "7", "7.0", "8", "8.0", "8.1", "8.2", "9", "9.0", "9.1", "9.2",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    #[serde(default)]
    pub(crate) minimum_build: u32,
    #[serde(default)]
    pub(crate) localization_target: Option<String>,
    #[serde(default)]
    pub(crate) metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) alternates: Vec<super::alternates::Alternate>,
    #[serde(default)]
    pub(crate) multilists: BTreeMap<i32, Vec<String>>,
    #[serde(default)]
    pub(crate) base_multilists: BTreeMap<i32, Vec<String>>,
    pub(crate) target: Target,
    pub(crate) version: String,
    pub(crate) name: String,
    pub(crate) author: String,
    pub(crate) description: String,
    pub(crate) website: Option<String>,
    pub(crate) format: String,
    pub(crate) dlc: Vec<(String, String)>,
    pub(crate) basegame: Vec<(String, String)>,
    pub(crate) structured: bool,
    pub(crate) merges: Vec<String>,
    pub(crate) embedded_tlk: bool,
    pub(crate) texture_runtime: bool,
    pub(crate) required_dlc: Vec<String>,
    pub(crate) incompatible_dlc: Vec<String>,
    pub(crate) obsolete_dlc: Vec<String>,
}

impl Manifest {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= 1024 * 1024,
            "moddesc.ini exceeds the 1 MiB limit"
        );
        let sections = sections(text)?;
        let get = |section: &str, key: &str| {
            sections
                .get(section)
                .and_then(|values| values.get(key))
                .map(String::as_str)
        };
        let required = |section: &str, key: &str| -> Result<String> {
            get(section, key)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .with_context(|| format!("moddesc.ini is missing [{section}] {key}"))
        };
        let format = required("modmanager", "cmmver")?;
        ensure!(
            FORMATS.contains(&format.as_str()),
            "Unsupported moddesc.ini cmmver '{format}'; this format needs compatibility review"
        );
        let target = Target::parse(&required("modinfo", "game")?)?;
        let minimum_build = get("modmanager", "minbuild")
            .map(|value| {
                value
                    .parse::<u32>()
                    .context("Invalid minimum manager build")
            })
            .transpose()?
            .unwrap_or_default();
        ensure!(
            minimum_build <= 137,
            "This manifest requires a newer manager build; its semantics need compatibility review"
        );
        let mut metadata = BTreeMap::new();
        for section in ["modmanager", "modinfo", "updates"] {
            if let Some(values) = sections.get(section) {
                for (key, value) in values {
                    metadata.insert(format!("{section}.{key}"), value.clone());
                }
            }
        }
        for key in [
            "sortalternates",
            "requiresenhancedbink",
            "batchinstallreversesort",
            "prefercompressed",
            "amdprocessoronly",
        ] {
            if let Some(value) = get("modinfo", key) {
                ensure!(
                    value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false"),
                    "Invalid ModInfo {key} boolean"
                );
            }
        }
        let mut unsupported = BTreeSet::new();
        for (section, values) in &sections {
            let allowed: &[&str] = match section.as_str() {
                "modmanager" => &["cmmver", "minbuild", "importedby"],
                "modinfo" => &[
                    "unofficial",
                    "updatecode",
                    "modid",
                    "compiledagainst",
                    "amdprocessoronly",
                    "prefercompressed",
                    "sortalternates",
                    "postinstalltool",
                    "requiresenhancedbink",
                    "batchinstallreversesort",
                    "game",
                    "modname",
                    "moddesc",
                    "modver",
                    "moddev",
                    "modsite",
                    "nexuscode",
                    "bannerimagename",
                    "requireddlc",
                    "incompatibledlc",
                ],
                "customdlc" => &[
                    "requiredcustomdlc",
                    "incompatiblecustomdlc",
                    "sourcedirs",
                    "destdirs",
                    "outdatedcustomdlc",
                    "altfiles",
                    "altdlc",
                ],
                "basegame" => &[
                    "altfiles",
                    "jobdescription",
                    "moddir",
                    "newfiles",
                    "replacefiles",
                    "mergemods",
                    "addfiles",
                    "addfilestargets",
                    "gamedirectorystructure",
                ],
                "game1_embedded_tlk" => &["usesfeature"],
                "localization" => &["files", "dlcname"],
                "updates" => &[
                    "nexusupdatecheck",
                    "serverfolder",
                    "blacklistedfiles",
                    "additionaldeploymentfolders",
                    "additionaldeploymentfiles",
                ],
                "asimods"
                    if get("asimods", "asimodstoinstall").is_some_and(|value| {
                        let compact: String =
                            value.chars().filter(|ch| !ch.is_whitespace()).collect();
                        let group = match target {
                            Target::Le1 => 88,
                            Target::Le2 => 89,
                            Target::Le3 => 87,
                        };
                        compact.eq_ignore_ascii_case(&format!("((GroupID={group}))"))
                            && format == "9.2"
                    }) =>
                {
                    &["asimodstoinstall"]
                }
                _ => {
                    unsupported.insert(format!("[{section}]"));
                    continue;
                }
            };
            for key in values.keys() {
                if !(allowed.contains(&key.as_str())
                    || matches!(section.as_str(), "customdlc" | "basegame")
                        && key.strip_prefix("multilist").is_some_and(|id| {
                            !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
                        }))
                {
                    unsupported.insert(format!("[{section}] {key}"));
                }
            }
        }
        ensure!(
            unsupported.is_empty(),
            "Unsupported moddesc.ini features: {}",
            unsupported.into_iter().collect::<Vec<_>>().join(", ")
        );
        if let Some(value) = get("updates", "nexusupdatecheck") {
            ensure!(
                value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false"),
                "Invalid UPDATES nexusupdatecheck boolean"
            );
        }
        let read_lists = |section: &str| -> Result<BTreeMap<i32, Vec<String>>> {
            let mut multilists = BTreeMap::new();
            if let Some(fields) = sections.get(section) {
                for (key, value) in fields {
                    if let Some(id) = key.strip_prefix("multilist") {
                        let id: i32 = id.parse().context("Invalid multilist ID")?;
                        let entries = list(Some(value))?;
                        ensure!(
                            !entries.is_empty() && entries.len() <= 100_000,
                            "Invalid multilist length"
                        );
                        let unique: BTreeSet<_> =
                            entries.iter().map(|path| path.to_lowercase()).collect();
                        ensure!(
                            unique.len() == entries.len()
                                && multilists.insert(id, entries).is_none(),
                            "Duplicate multilist ID or file"
                        );
                    }
                }
            }
            Ok(multilists)
        };
        let multilists = read_lists("customdlc")?;
        let base_multilists = read_lists("basegame")?;
        let mut alternates =
            super::alternates::Alternate::parse(get("customdlc", "altfiles"), false, target)?;
        alternates.extend(super::alternates::Alternate::parse(
            get("customdlc", "altdlc"),
            true,
            target,
        )?);
        alternates.extend(super::alternates::Alternate::parse_in(
            get("basegame", "altfiles"),
            false,
            target,
            true,
        )?);
        let dlc = paired(
            get("customdlc", "sourcedirs"),
            get("customdlc", "destdirs"),
            "CUSTOMDLC",
        )?;
        for (_, destination) in &dlc {
            validate_dlc(destination)?;
            ensure!(
                !target.is_official_dlc(destination),
                "CUSTOMDLC cannot replace official DLC '{destination}'"
            );
        }
        let directory = relative_directory(get("basegame", "moddir").unwrap_or("."))?;
        let structured = match get("basegame", "gamedirectorystructure") {
            None => false,
            Some(value) if value.eq_ignore_ascii_case("true") => true,
            Some(value) if value.eq_ignore_ascii_case("false") => false,
            Some(value) => bail!("Invalid BASEGAME gamedirectorystructure '{value}'"),
        };
        let replacements = file_pairs(
            get("basegame", "newfiles"),
            get("basegame", "replacefiles"),
            structured,
        )?;
        let additions = file_pairs(
            get("basegame", "addfiles"),
            get("basegame", "addfilestargets"),
            false,
        )?;
        let mut basegame: Vec<_> = replacements
            .into_iter()
            .chain(additions.into_iter().filter(|_| !structured))
            .map(|(source, destination)| {
                let source = join_directory(&directory, &source);
                (source, destination)
            })
            .collect();
        let merges = list(get("basegame", "mergemods"))?;
        for name in &merges {
            ensure!(
                !name.contains(['/', '\\']) && name.to_ascii_lowercase().ends_with(".m3m"),
                "A merge job must name an .m3m file in MergeMods"
            );
        }
        let embedded_tlk = match get("game1_embedded_tlk", "usesfeature") {
            None | Some("false") => false,
            Some("true") => true,
            Some(value) => bail!("Invalid GAME1_EMBEDDED_TLK usesfeature '{value}'"),
        };
        ensure!(
            !embedded_tlk || target == Target::Le1,
            "GAME1_EMBEDDED_TLK requires LE1"
        );
        let obsolete_dlc = list(get("customdlc", "outdatedcustomdlc"))?;
        ensure!(
            obsolete_dlc.len() <= 1024,
            "Too many obsolete DLC declarations"
        );
        let mut retired = BTreeSet::new();
        for name in &obsolete_dlc {
            validate_dlc(name)?;
            ensure!(
                !target.is_official_dlc(name),
                "Official DLC cannot be marked obsolete: '{name}'"
            );
            ensure!(
                retired.insert(name.to_lowercase()),
                "Duplicate obsolete DLC '{name}'"
            );
        }
        let localization = get("localization", "files");
        if let Some(files) = localization {
            ensure!(
                target != Target::Le1
                    && dlc.is_empty()
                    && basegame.is_empty()
                    && merges.is_empty()
                    && !embedded_tlk
                    && alternates.is_empty()
                    && obsolete_dlc.is_empty(),
                "LOCALIZATION is an exclusive LE2/LE3 installation job"
            );
            let dlc = required("localization", "dlcname")?;
            validate_dlc(&dlc)?;
            ensure!(
                !target.is_official_dlc(&dlc),
                "LOCALIZATION must target a custom DLC"
            );
            for source in list(Some(files))? {
                let name = source
                    .rsplit('/')
                    .next()
                    .context("Missing localization filename")?;
                ensure!(
                    name.to_ascii_lowercase().ends_with(".tlk"),
                    "LOCALIZATION only supports TLK files"
                );
                let target = format!("BioGame/DLC/{dlc}/CookedPCConsole/{name}");
                basegame.push((source, target));
            }
        }
        ensure!(
            !dlc.is_empty()
                || !basegame.is_empty()
                || !merges.is_empty()
                || embedded_tlk
                || !obsolete_dlc.is_empty()
                || !alternates.is_empty(),
            "moddesc.ini declares no installation operations"
        );
        let mut required_dlc = list(get("modinfo", "requireddlc"))?;
        required_dlc.extend(list(get("customdlc", "requiredcustomdlc"))?);
        if localization.is_some() {
            required_dlc.push(required("localization", "dlcname")?);
        }
        let mut incompatible_dlc = list(get("modinfo", "incompatibledlc"))?;
        incompatible_dlc.extend(list(get("customdlc", "incompatiblecustomdlc"))?);
        for name in required_dlc.iter().chain(&incompatible_dlc) {
            super::dependency::Dependency::parse(name, &format)?;
        }
        Ok(Self {
            minimum_build,
            localization_target: localization
                .map(|_| required("localization", "dlcname"))
                .transpose()?,
            metadata,
            alternates,
            multilists,
            base_multilists,
            texture_runtime: get("asimods", "asimodstoinstall").is_some(),
            target,
            format,
            version: required("modinfo", "modver")?,
            name: required("modinfo", "modname")?,
            author: required("modinfo", "moddev")?,
            description: required("modinfo", "moddesc")?,
            website: get("modinfo", "modsite").map(str::to_owned),
            dlc,
            basegame,
            structured,
            merges,
            embedded_tlk,
            required_dlc,
            incompatible_dlc,
            obsolete_dlc,
        })
    }
}

pub(super) fn sections(text: &str) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let mut sections = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut current = None;
    for (index, line) in text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .lines()
        .enumerate()
    {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        ensure!(
            !line.contains('\0'),
            "Invalid moddesc.ini line {}: embedded NUL",
            index + 1
        );
        if line.starts_with('[') {
            let section = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
                .context("Invalid moddesc.ini section")?
                .trim()
                .to_ascii_lowercase();
            ensure!(
                !section.is_empty() && !section.contains(['[', ']']),
                "Invalid moddesc.ini section"
            );
            ensure!(
                sections.insert(section.clone(), BTreeMap::new()).is_none(),
                "Duplicate moddesc.ini section [{section}]"
            );
            current = Some(section);
        } else {
            let (key, value) = line.split_once('=').with_context(|| {
                format!(
                    "Invalid moddesc.ini line {}: expected a section or key=value",
                    index + 1
                )
            })?;
            let key = key.trim().to_ascii_lowercase();
            ensure!(
                !key.is_empty(),
                "Invalid moddesc.ini line {}: empty key",
                index + 1
            );
            let section = current
                .as_ref()
                .context("moddesc.ini contains keys outside a section")?;
            ensure!(
                sections
                    .get_mut(section)
                    .context("Missing moddesc.ini section")?
                    .insert(key.clone(), value.trim().into())
                    .is_none(),
                "Duplicate moddesc.ini key [{section}] {key}"
            );
        }
    }
    Ok(sections)
}

pub(super) fn relative_directory(value: &str) -> Result<String> {
    let value = value.trim_end_matches(['/', '\\']);
    if value == "." {
        Ok(value.into())
    } else {
        relative_path(value)
    }
}

pub(super) fn join_directory(directory: &str, relative: &str) -> String {
    match (directory, relative) {
        (".", _) => relative.into(),
        (_, ".") => directory.into(),
        _ => format!("{directory}/{relative}"),
    }
}

pub(super) fn file_pairs(
    sources: Option<&str>,
    destinations: Option<&str>,
    directories: bool,
) -> Result<Vec<(String, String)>> {
    let parse = |value: Option<&str>| -> Result<Vec<String>> {
        value
            .into_iter()
            .flat_map(|value| value.split(';'))
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                if directories {
                    relative_directory(entry)
                } else {
                    relative_path(entry)
                }
            })
            .collect()
    };
    let sources = parse(sources)?;
    let destinations = parse(destinations)?;
    ensure!(
        sources.len() == destinations.len(),
        "BASEGAME source and destination lists have different lengths"
    );
    Ok(sources.into_iter().zip(destinations).collect())
}

pub(super) fn relative_path(value: &str) -> Result<String> {
    let normalized = value.replace('\\', "/");
    ensure!(
        !normalized.is_empty() && !normalized.contains([':', '\0']) && !normalized.starts_with('/'),
        "Invalid package path '{value}'"
    );
    for component in normalized.split('/') {
        let basename = component
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        ensure!(
            !matches!(
                basename.as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            ) && !component
                .chars()
                .any(|ch| ch.is_control() || "<>\"|?*".contains(ch)),
            "Invalid Windows package path '{value}'"
        );
        ensure!(
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.ends_with(['.', ' ']),
            "Invalid package path '{value}'"
        );
    }
    Ok(normalized)
}

pub(super) fn validate_dlc(name: &str) -> Result<()> {
    let normalized = relative_path(name)?;
    ensure!(
        !normalized.contains('/')
            && normalized.to_ascii_uppercase().starts_with("DLC_")
            && normalized.len() > 4,
        "Expected a single DLC_* directory, got '{name}'"
    );
    Ok(())
}

fn list(value: Option<&str>) -> Result<Vec<String>> {
    match value {
        None => Ok(Vec::new()),
        Some(value) => value
            .split(';')
            .map(|entry| relative_path(entry.trim()))
            .collect(),
    }
}

fn paired(
    sources: Option<&str>,
    destinations: Option<&str>,
    section: &str,
) -> Result<Vec<(String, String)>> {
    let sources = list(sources)?;
    let destinations = list(destinations)?;
    ensure!(
        sources.len() == destinations.len(),
        "[{section}] source and destination lists have different lengths"
    );
    Ok(sources.into_iter().zip(destinations).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic() -> String {
        "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE1\nmodname=Example\nmoddesc=Example content\nmodver=1.0\nmoddev=Example author\n[CUSTOMDLC]\nsourcedirs=Content\\DLC_MOD_EXAMPLE\ndestdirs=DLC_MOD_EXAMPLE\n".into()
    }

    #[test]
    fn preserves_windows_paths_and_manifest_metadata() -> Result<()> {
        let manifest = Manifest::parse(&basic())?;
        assert_eq!(manifest.target, Target::Le1);
        assert_eq!(
            manifest.dlc,
            vec![("Content/DLC_MOD_EXAMPLE".into(), "DLC_MOD_EXAMPLE".into())]
        );
        assert_eq!(manifest.version, "1.0");
        Ok(())
    }

    #[test]
    fn parses_additions_and_structured_directory_pairs() -> Result<()> {
        let text = basic()
            + "[BASEGAME]\nmoddir=Content\\\nnewfiles=one.pcc;\nreplacefiles=BioGame/CookedPCConsole/one.pcc;\naddfiles=two.pcc\naddfilestargets=BioGame/CookedPCConsole/two.pcc\n";
        let manifest = Manifest::parse(&text)?;
        assert!(!manifest.structured);
        assert_eq!(manifest.basegame.len(), 2);
        assert_eq!(manifest.basegame[1].0, "Content/two.pcc");
        let text = basic()
            + "[BASEGAME]\nmoddir=Content\ngamedirectorystructure=TRUE\nnewfiles=.\\\nreplacefiles=BioGame/\n";
        let manifest = Manifest::parse(&text)?;
        assert!(manifest.structured);
        assert_eq!(
            manifest.basegame,
            vec![("Content".into(), "BioGame".into())]
        );
        for suffix in [
            "[BASEGAME]\ngamedirectorystructure=maybe\n",
            "[BASEGAME]\naddfiles=one.pcc\n",
            "[BASEGAME]\naddfilestargets=BioGame/one.pcc\n",
            "[BASEGAME]\nnewfiles=.\nreplacefiles=.\n",
            "[BASEGAME]\ngamedirectorystructure=true\nnewfiles=../\nreplacefiles=BioGame\n",
        ] {
            assert!(Manifest::parse(&(basic() + suffix)).is_err(), "{suffix}");
        }
        Ok(())
    }

    #[test]
    fn rejects_duplicate_keys_sections_and_malformed_lines() {
        for suffix in [
            "destdirs=DLC_OTHER\n",
            "DESTDIRS=DLC_OTHER\n",
            "[CUSTOMDLC]\n",
            "broken line\n",
            "=empty key\n",
            "[CUSTOMDLC] trailing text\n",
            "destdirs=bad\0value\n",
        ] {
            assert!(Manifest::parse(&(basic() + suffix)).is_err(), "{suffix}");
        }
    }

    #[test]
    fn preserves_literal_manifest_values_and_windows_line_endings() -> Result<()> {
        let text = basic().replace(
            "moddesc=Example content",
            r#"moddesc="Literal"; # [Option]=C:\folder\n"#,
        );
        let manifest = Manifest::parse(&format!("\u{feff}{}", text.replace('\n', "\r\n")))?;
        assert_eq!(manifest.description, r#""Literal"; # [Option]=C:\folder\n"#);
        assert!(Manifest::parse(&("outside=value\n".to_string() + &basic())).is_err());
        Ok(())
    }

    #[test]
    fn retains_update_metadata_but_rejects_unknown_instructions() -> Result<()> {
        Manifest::parse(&(basic() + "[UPDATES]\n;additionaldeploymentfolders=Authoring\n"))?;
        let manifest =
            Manifest::parse(&(basic() + "[UPDATES]\nadditionaldeploymentfolders=Authoring\n"))?;
        assert_eq!(
            manifest
                .metadata
                .get("updates.additionaldeploymentfolders")
                .map(String::as_str),
            Some("Authoring")
        );
        for section in [
            "[UPDATES]\nfutureinstruction=Authoring\n",
            "[FutureFeature]\n",
        ] {
            assert!(Manifest::parse(&(basic() + section)).is_err());
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn accepts_only_game_matching_texture_runtime_groups_and_known_update_metadata() -> Result<()> {
        for (game, group) in [("LE1", 88), ("LE2", 89), ("LE3", 87)] {
            let text = basic()
                .replace("cmmver=9.1", "cmmver=9.2")
                .replace("game=LE1", &format!("game={game}"));
            let manifest = Manifest::parse(&format!(
                "{text}[UPDATES]\nnexusupdatecheck=false\n[ASIMODS]\nasimodstoinstall=((GroupID={group}))\n"
            ))?;
            assert!(manifest.texture_runtime);
            for invalid in [
                "((GroupID=1))",
                "((GroupID=88,Future=true))",
                "((GroupID=88),(GroupID=89))",
                "",
            ] {
                assert!(
                    Manifest::parse(&format!("{text}[ASIMODS]\nasimodstoinstall={invalid}\n"))
                        .is_err()
                );
            }
            assert!(
                Manifest::parse(&format!("{text}[UPDATES]\nnexusupdatecheck=perhaps\n")).is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn reports_all_unimplemented_manifest_operations() {
        let error = Manifest::parse(&(basic() + "futureoperation=Future\noutdatedcustomdlc=DLC_OLD\n[ASIMODS]\nasimodstoinstall=((GroupID=1))\n")).unwrap_err().to_string();
        for feature in ["futureoperation", "asimods"] {
            assert!(error.contains(feature));
        }
    }

    #[test]
    fn rejects_future_versions_mismatched_lists_and_foreign_games() {
        for (old, new) in [
            ("9.1", "9.3"),
            ("game=LE1", "game=ME1"),
            ("destdirs=DLC_MOD_EXAMPLE", "destdirs=DLC_A;DLC_B"),
        ] {
            assert!(Manifest::parse(&basic().replace(old, new)).is_err());
        }
    }

    #[test]
    fn validates_obsolete_dlc_without_allowing_official_content_or_foreign_paths() -> Result<()> {
        let manifest = Manifest::parse(&(basic() + "outdatedcustomdlc=DLC_OLD;DLC_OTHER\n"))?;
        assert_eq!(manifest.obsolete_dlc, ["DLC_OLD", "DLC_OTHER"]);
        for name in [
            "../DLC_OLD",
            "DLC_OLD/child",
            "DLC_OLD;dlc_old",
            "C:\\DLC_OLD",
            "DLC_UPD_Patch01",
        ] {
            let text =
                basic().replace("game=LE1", "game=LE3") + &format!("outdatedcustomdlc={name}\n");
            assert!(Manifest::parse(&text).is_err(), "{name}");
        }
        Ok(())
    }

    #[test]
    fn rejects_unsafe_windows_and_unix_paths() {
        for path in [
            "../file",
            "a/../file",
            "a\\..\\file",
            "C:\\file",
            "\\\\server\\file",
            "/file",
            "file:stream",
            "a//file",
            "a/./file",
            "a./file",
            "a /file",
            "aux.txt",
            "CON",
            "dir/LPT1.pcc",
            "file?.pcc",
            "a\tfile",
        ] {
            assert!(relative_path(path).is_err(), "{path}");
        }
    }
}
