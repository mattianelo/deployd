/// Extract the ten-digit Nexus CDN timestamp appended to a downloaded filename.
pub(crate) fn extract_nexus_timestamp(filename: &str) -> Option<i64> {
    let stem = filename
        .rsplit_once('.')
        .map(|(left, _)| left)
        .unwrap_or(filename);
    nexus_timestamp_suffix(stem)?.parse().ok()
}

/// Normalize a Nexus filename for comparison with API-provided file names.
pub(crate) fn normalize_nexus_filename(filename: &str) -> String {
    let stem = filename
        .rsplit_once('.')
        .map(|(left, _)| left)
        .unwrap_or(filename);
    nexus_timestamp_suffix(stem)
        .and_then(|timestamp| stem.strip_suffix(timestamp))
        .and_then(|prefix| prefix.strip_suffix('-'))
        .unwrap_or(stem)
        .to_string()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CurrentNexusFileIdentity {
    pub(crate) label: String,
    pub(crate) mod_id: i64,
    pub(crate) version: String,
}

pub(crate) fn current_nexus_file_identity(filename: &str) -> Option<CurrentNexusFileIdentity> {
    let stem = filename
        .rsplit_once('.')
        .map(|(left, _)| left)
        .unwrap_or(filename);
    let tokens: Vec<_> = stem.split_whitespace().collect();
    let timestamp_index = tokens
        .iter()
        .position(|token| is_nexus_download_timestamp(token))?;
    if timestamp_index < 2 || timestamp_index + 2 != tokens.len() {
        return None;
    }
    let download_token = tokens[timestamp_index + 1];
    if !download_token
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        return None;
    }
    let mod_id = tokens[timestamp_index - 2].parse::<i64>().ok()?;
    if mod_id <= 0 || mod_id >= 1_000_000_000 {
        return None;
    }
    let label = tokens[..timestamp_index - 2].join(" ");
    let version = tokens[timestamp_index - 1].to_string();
    (!label.is_empty() && !version.is_empty()).then_some(CurrentNexusFileIdentity {
        label,
        mod_id,
        version,
    })
}

fn nexus_timestamp_suffix(value: &str) -> Option<&str> {
    let (_, suffix) = value.rsplit_once('-')?;
    (suffix.len() == 10 && suffix.bytes().all(|byte| byte.is_ascii_digit())).then_some(suffix)
}

/// Filename numbers can also be versions; this hint requires verification before a page lookup.
pub(crate) fn parse_nexus_mod_id(filename: &str) -> Option<i64> {
    let tokens: Vec<_> = filename.split_whitespace().collect();
    for (timestamp_index, token) in tokens.iter().enumerate() {
        if timestamp_index >= 2 && is_nexus_download_timestamp(token) {
            let id = tokens[timestamp_index - 2].parse::<i64>().ok()?;
            return (id > 0 && id < 1_000_000_000).then_some(id);
        }
    }

    if let Ok(pattern) = regex::Regex::new(r"-(\d+)-")
        && let Some(captures) = pattern.captures(filename)
        && let Some(id) = captures
            .get(1)
            .and_then(|capture| capture.as_str().parse::<i64>().ok())
        && id > 0
        && id < 1_000_000_000
    {
        return Some(id);
    }

    let stem = filename
        .rsplit_once('.')
        .map(|(left, _)| left)
        .unwrap_or(filename);
    stem.split('-').find_map(|part| {
        (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse::<i64>().ok())
            .flatten()
            .filter(|id| *id > 0 && *id < 1_000_000_000)
    })
}

fn is_nexus_download_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 17
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b'-'
        && bytes[16] == b'Z'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit())
}

pub(crate) fn parse_nexus_identity_input(
    raw: &str,
    fallback_domain: &str,
) -> Result<crate::models::download::NexusIds, String> {
    let raw = raw.trim();
    let (mod_id, domain) = if let Ok(id) = raw.parse::<i64>() {
        (id, fallback_domain.to_string())
    } else {
        let url = reqwest::Url::parse(raw)
            .map_err(|_| "Enter a positive mod ID or a Nexus mod-page URL.")?;
        if !matches!(url.scheme(), "https" | "http")
            || !matches!(url.host_str(), Some("www.nexusmods.com" | "nexusmods.com"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err("Use a mod-page URL on nexusmods.com.".into());
        }
        let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
        let [domain, "mods", id] = parts.as_slice() else {
            return Err("Use a Nexus URL with /game/mods/mod-ID.".into());
        };
        let id = id
            .parse::<i64>()
            .map_err(|_| "The Nexus mod ID must be a positive number.")?;
        (id, domain.to_ascii_lowercase())
    };
    if mod_id <= 0 {
        return Err("The Nexus mod ID must be a positive number.".into());
    }
    if !crate::core::game::all_nexus_domains().contains(&domain.as_str()) {
        return Err("This Nexus game is not supported by Deployd.".into());
    }
    Ok(crate::models::download::NexusIds {
        mod_id,
        file_id: 0,
        domain,
    })
}

/// Parse a positive Nexus mod ID from a bare number or Nexus URL.
pub(crate) fn parse_nexus_mod_id_from_input(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if let Ok(id) = raw.parse::<i64>() {
        return (id > 0).then_some(id);
    }

    let path = raw.split(['?', '#']).next().unwrap_or(raw);
    path.trim_end_matches('/')
        .rsplit('/')
        .find_map(|segment| segment.parse::<i64>().ok().filter(|id| *id > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nexus_url_selects_its_own_game_and_ignores_file_query_parameters() {
        let ids = parse_nexus_identity_input(
            "https://www.nexusmods.com/masseffectlegendaryedition/mods/23?tab=files&file_id=999",
            "fallout4",
        )
        .unwrap();
        assert_eq!(ids.mod_id, 23);
        assert_eq!(ids.file_id, 0);
        assert_eq!(ids.domain, "masseffectlegendaryedition");
        assert_eq!(
            parse_nexus_identity_input("8", "masseffectlegendaryedition")
                .unwrap()
                .mod_id,
            8
        );
    }

    #[test]
    fn identity_input_rejects_unrelated_urls_and_unsupported_games() {
        for raw in [
            "0",
            "-1",
            "https://example.com/fallout4/mods/12631",
            "https://www.nexusmods.com.evil.example/fallout4/mods/12631",
            "https://www.nexusmods.com/fallout4/files/12631",
            "https://www.nexusmods.com/unknown/mods/1",
            "https://user:password@www.nexusmods.com/fallout4/mods/12631",
        ] {
            assert!(
                parse_nexus_identity_input(raw, "fallout4").is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn extracts_only_ten_digit_nexus_timestamp_suffixes() {
        assert_eq!(
            extract_nexus_timestamp("mod-1756684569.7z"),
            Some(1_756_684_569)
        );
        assert_eq!(extract_nexus_timestamp("mod-175668456.7z"), None);
        assert_eq!(extract_nexus_timestamp("mod-175668456x.7z"), None);
    }

    #[test]
    fn normalizes_only_nexus_timestamp_suffixes() {
        assert_eq!(normalize_nexus_filename("foo-1.0-1756684569.7z"), "foo-1.0");
        assert_eq!(
            normalize_nexus_filename("foo-1.0-175668456.7z"),
            "foo-1.0-175668456"
        );
        assert_eq!(normalize_nexus_filename("foo-1.0.zip"), "foo-1.0");
    }

    #[test]
    fn parses_current_nexus_file_identity() {
        assert_eq!(
            current_nexus_file_identity(
                "Dynamic Grass 108480 1.3.0 2026-08-31T12-00Z Gpr9A6gVu.zip"
            ),
            Some(CurrentNexusFileIdentity {
                label: "Dynamic Grass".to_string(),
                mod_id: 108_480,
                version: "1.3.0".to_string(),
            })
        );
        assert_eq!(
            current_nexus_file_identity(
                "RobCo Patcher - RD 69798 6.0.5 2026-08-26T23-09Z z23PjGUj8.zip"
            ),
            Some(CurrentNexusFileIdentity {
                label: "RobCo Patcher - RD".to_string(),
                mod_id: 69_798,
                version: "6.0.5".to_string(),
            })
        );
        assert_eq!(
            current_nexus_file_identity(
                "Fancy Prefabs 1.0.0 107091 1 2026-07-18T19-18Z z23PjGFON.zip"
            ),
            Some(CurrentNexusFileIdentity {
                label: "Fancy Prefabs 1.0.0".to_string(),
                mod_id: 107_091,
                version: "1".to_string(),
            })
        );
        assert_eq!(
            current_nexus_file_identity(
                "Release 108480 1.3.0 2026-08-31T12-00Z Gpr9A6gVu notes.zip"
            ),
            None
        );
    }

    // @variants: both
    #[test]
    fn parses_short_mele_mod_ids_from_downloaded_archives() {
        assert_eq!(
            parse_nexus_mod_id(
                "Unofficial Mass Effect 2 Legendary Edition Patch-8-0-9-6-1762432362.7z"
            ),
            Some(8)
        );
        assert_eq!(
            parse_nexus_mod_id("LE1 Community Patch-23-2-0-1762480826.7z"),
            Some(23)
        );
    }

    // @variants: both
    #[test]
    fn parses_page_id_instead_of_cdn_timestamp() {
        assert_eq!(
            parse_nexus_mod_id("NEO-65761-3-1-1-1763043682"),
            Some(65_761)
        );
    }

    #[test]
    fn parses_mod_id_from_current_nexus_filename() {
        assert_eq!(
            parse_nexus_mod_id("Dynamic Grass 108480 1.3.0 2026-08-31T12-00Z Gpr9A6gVu.zip"),
            Some(108_480)
        );
        assert_eq!(
            parse_nexus_mod_id("RobCo Patcher - RD 69798 6.0.5 2026-08-26T23-09Z z23PjGUj8.zip"),
            Some(69_798)
        );
    }

    #[test]
    fn does_not_treat_current_filename_month_as_mod_id() {
        assert_ne!(
            parse_nexus_mod_id("Dynamic Grass 108480 1.3.0 2026-08-31T12-00Z Gpr9A6gVu.zip"),
            Some(8)
        );
    }

    #[test]
    fn parses_bare_and_url_nexus_mod_ids() {
        assert_eq!(parse_nexus_mod_id_from_input(" 101 "), Some(101));
        assert_eq!(
            parse_nexus_mod_id_from_input(
                "https://www.nexusmods.com/skyrimspecialedition/mods/12604?tab=files"
            ),
            Some(12_604)
        );
    }

    #[test]
    fn rejects_invalid_nexus_mod_ids() {
        assert_eq!(parse_nexus_mod_id_from_input(""), None);
        assert_eq!(parse_nexus_mod_id_from_input("0"), None);
        assert_eq!(parse_nexus_mod_id_from_input("-1"), None);
        assert_eq!(parse_nexus_mod_id_from_input("not a nexus id"), None);
    }
}
