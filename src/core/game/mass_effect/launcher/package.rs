use super::*;

pub(crate) async fn inspect(archive: PathBuf) -> Result<Inspected> {
    tokio::task::spawn_blocking(move || {
        let source = crate::core::archive::extract_archive(&archive, None)?;
        let fallback = archive
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Launcher mod");
        let entry = parse(source.path(), fallback)?;
        Ok(Inspected {
            entry,
            source: Some(source),
        })
    })
    .await
    .context("Launcher archive inspection worker failed")?
}

pub(in crate::core::game::mass_effect) fn parse(root: &Path, fallback: &str) -> Result<Entry> {
    let sources = super::super::package::scan(root)?;
    let manifests: Vec<_> = sources
        .iter()
        .filter(|source| {
            source
                .relative
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("moddesc.ini"))
        })
        .collect();
    ensure!(
        manifests.len() <= 1,
        "Select an archive containing one launcher mod"
    );
    let mut files = Vec::new();
    let mut name = fallback.to_owned();
    if let Some(manifest) = manifests.first() {
        let bytes = super::super::m3za::read_input(root, manifest, 1024 * 1024)?;
        let text = std::str::from_utf8(&bytes).context("Launcher manifest must be UTF-8")?;
        let sections = super::super::manifest::sections(text)?;
        let field = |section: &str, key: &str| {
            sections
                .get(section)
                .and_then(|values| values.get(key))
                .map(String::as_str)
        };
        ensure!(
            field("modinfo", "game").is_some_and(|value| value.eq_ignore_ascii_case("LELAUNCHER")),
            "This archive does not target the shared MELE launcher"
        );
        ensure!(
            field("modmanager", "cmmver")
                .is_some_and(|value| super::super::manifest::FORMATS.contains(&value)),
            "Unsupported launcher manifest version"
        );
        ensure!(
            field("modmanager", "minbuild")
                .map(str::parse::<u32>)
                .transpose()?
                .is_none_or(|build| build <= 137),
            "Launcher manifest requires newer installer semantics"
        );
        for (section, values) in &sections {
            ensure!(
                ["modmanager", "modinfo", "updates", "basegame"].contains(&section.as_str()),
                "Unsupported launcher manifest section '{section}'"
            );
            let metadata: &[&str] = match section.as_str() {
                "modmanager" => &["cmmver", "minbuild", "importedby"],
                "modinfo" => &[
                    "game",
                    "modname",
                    "modver",
                    "moddev",
                    "moddesc",
                    "modsite",
                    "nexuscode",
                    "modid",
                    "updatecode",
                    "unofficial",
                    "compiledagainst",
                    "requiresenhancedbink",
                ],
                "updates" => &[
                    "updatecode",
                    "serverfolder",
                    "blacklist",
                    "additionaldeploymentfolders",
                ],
                _ => &[],
            };
            if !metadata.is_empty() {
                ensure!(
                    values.keys().all(|key| metadata.contains(&key.as_str())),
                    "Unsupported launcher manifest metadata or requirements"
                );
            }
            if section == "basegame" {
                ensure!(
                    values.keys().all(|key| [
                        "moddir",
                        "newfiles",
                        "replacefiles",
                        "addfiles",
                        "addfilestargets",
                        "gamedirectorystructure"
                    ]
                    .contains(&key.as_str())),
                    "Unsupported launcher installer operation"
                );
            }
        }
        name = field("modinfo", "modname")
            .context("Launcher mod has no name")?
            .into();
        let wrapper = manifest
            .relative
            .rsplit_once('/')
            .map(|(parent, _)| format!("{parent}/"))
            .unwrap_or_default();
        let directory =
            super::super::manifest::relative_directory(field("basegame", "moddir").unwrap_or("."))?;
        if directory != "." {
            super::super::baseline::relative(&directory)?;
        }
        let prefix = format!(
            "{wrapper}{}",
            if directory == "." {
                String::new()
            } else {
                format!("{directory}/")
            }
        );
        let structured = field("basegame", "gamedirectorystructure").unwrap_or("false");
        ensure!(
            ["true", "false"].contains(&structured.to_ascii_lowercase().as_str()),
            "Invalid launcher structured layout"
        );
        let structured = structured.eq_ignore_ascii_case("true");
        let replacements = super::super::manifest::file_pairs(
            field("basegame", "newfiles"),
            field("basegame", "replacefiles"),
            structured,
        )?;
        let additions = super::super::manifest::file_pairs(
            field("basegame", "addfiles"),
            field("basegame", "addfilestargets"),
            false,
        )?;
        for (source, target) in replacements
            .into_iter()
            .chain(additions.into_iter().filter(|_| !structured))
        {
            if structured {
                let directory = if source == "." {
                    prefix.clone()
                } else {
                    format!("{prefix}{source}/")
                };
                let mut found = false;
                for file in &sources {
                    if file.relative == manifest.relative {
                        continue;
                    }
                    if file
                        .relative
                        .to_lowercase()
                        .starts_with(&directory.to_lowercase())
                    {
                        let suffix = &file.relative[directory.len()..];
                        let destination = super::super::manifest::join_directory(&target, suffix);
                        files.push(mapping(file, &destination)?);
                        found = true;
                    }
                }
                ensure!(
                    found,
                    "Structured launcher source folder is missing or empty"
                );
            } else {
                let file = sources
                    .iter()
                    .find(|file| {
                        file.relative
                            .eq_ignore_ascii_case(&format!("{prefix}{source}"))
                    })
                    .context("Launcher source file is missing")?;
                files.push(mapping(file, &target)?);
            }
        }
    } else {
        for source in &sources {
            let path = source
                .relative
                .strip_prefix("Game/Launcher/")
                .or_else(|| source.relative.strip_prefix("Launcher/"))
                .unwrap_or(&source.relative);
            let path = if !path.contains('/') && path.to_ascii_lowercase().ends_with(".asi") {
                format!("ASI/{path}")
            } else {
                path.into()
            };
            files.push(mapping(source, &path)?);
        }
    }
    ensure!(!files.is_empty(), "No supported launcher files were found");
    validate_payloads(root, &files, &sources)?;
    let hash = super::super::package::tree_digest(&sources);
    let entry = Entry {
        id: Uuid::new_v4().to_string(),
        name,
        enabled: true,
        source_sha256: hash,
        approval: String::new(),
        files,
        sources,
    };
    let mut validated = entry.clone();
    validated.approval = validated.source_sha256.clone();
    validate_entries(&[validated])?;
    Ok(entry)
}

fn mapping(source: &SourceFile, destination_path: &str) -> Result<Mapping> {
    destination(destination_path)?;
    if super::super::binary::executable(&source.relative) {
        ensure!(
            super::super::binary::executable(destination_path),
            "Plugins cannot be renamed as launcher content"
        );
    }
    Ok(Mapping {
        source: source.relative.clone(),
        destination: destination_path.into(),
        identity: Identity {
            size: source.size,
            sha256: source.sha256.clone(),
        },
    })
}

pub(super) fn validate_payloads(
    root: &Path,
    mappings: &[Mapping],
    sources: &[SourceFile],
) -> Result<()> {
    super::super::binary::inspect(root, sources)?;
    for mapping in mappings
        .iter()
        .filter(|mapping| super::super::binary::executable(&mapping.destination))
    {
        let source = sources
            .iter()
            .find(|source| source.relative == mapping.source)
            .context("Missing launcher plugin source")?;
        let bytes = super::super::m3za::read_input(root, source, 64 * 1024 * 1024)?;
        super::super::binary::pe(&bytes)?;
    }
    Ok(())
}
