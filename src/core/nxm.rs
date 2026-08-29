use anyhow::{Context, Result, bail};

fn validate_domain(domain: &str) -> Result<()> {
    if domain
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        bail!("invalid game domain in NXM link: {domain:?}")
    }
}

/// Parsed NXM link from the Nexus Mods "Download with Mod Manager" button.
///
/// Format: `nxm://<domain>/mods/<mod_id>/files/<file_id>?key=<key>&expires=<expires>`
#[derive(Debug, Clone)]
pub struct NxmLink {
    pub domain: String,
    pub mod_id: i64,
    pub file_id: i64,
    /// Download key (present for free users, optional for premium).
    pub key: Option<String>,
    /// Expiry timestamp for the download key.
    pub expires: Option<String>,
}

impl NxmLink {
    pub fn parse(uri: &str) -> Result<Self> {
        let stripped = uri.strip_prefix("nxm://").context("not an nxm:// link")?;

        // Split off query string
        let (path, query) = match stripped.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (stripped, None),
        };

        let parts: Vec<&str> = path.split('/').collect();
        // Expected: [domain, "mods", mod_id, "files", file_id]
        if parts.len() < 5 || parts[1] != "mods" || parts[3] != "files" {
            bail!("invalid NXM link format: {uri}");
        }

        let domain = parts[0].to_string();
        validate_domain(&domain)?;
        let mod_id: i64 = parts[2].parse().context("invalid mod ID in NXM link")?;
        let file_id: i64 = parts[4].parse().context("invalid file ID in NXM link")?;

        let mut key = None;
        let mut expires = None;

        if let Some(q) = query {
            for param in q.split('&') {
                if let Some((k, v)) = param.split_once('=') {
                    match k {
                        "key" => key = Some(v.to_string()),
                        "expires" => expires = Some(v.to_string()),
                        _ => {}
                    }
                }
            }
        }

        Ok(Self {
            domain,
            mod_id,
            file_id,
            key,
            expires,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_nxm_link() {
        let link =
            NxmLink::parse("nxm://skyrimspecialedition/mods/2347/files/12345?key=abc&expires=999")
                .expect("full NXM link should parse");
        assert_eq!(link.domain, "skyrimspecialedition");
        assert_eq!(link.mod_id, 2347);
        assert_eq!(link.file_id, 12345);
        assert_eq!(link.key.as_deref(), Some("abc"));
        assert_eq!(link.expires.as_deref(), Some("999"));
    }

    #[test]
    fn parse_nxm_link_without_key() {
        let link = NxmLink::parse("nxm://fallout4/mods/100/files/200")
            .expect("NXM link without a key should parse");
        assert_eq!(link.domain, "fallout4");
        assert_eq!(link.mod_id, 100);
        assert_eq!(link.file_id, 200);
        assert!(link.key.is_none());
        assert!(link.expires.is_none());
    }

    #[test]
    fn parse_invalid_link() {
        assert!(NxmLink::parse("https://nexusmods.com/foo").is_err());
        assert!(NxmLink::parse("nxm://domain/bad/format").is_err());
    }

    #[test]
    fn rejects_domain_with_path_traversal() {
        assert!(NxmLink::parse("nxm://../../evil/mods/1/files/1").is_err());
    }

    #[test]
    fn rejects_domain_with_special_chars() {
        assert!(NxmLink::parse("nxm://bad domain/mods/1/files/1").is_err());
        assert!(NxmLink::parse("nxm://bad%20domain/mods/1/files/1").is_err());
    }
}
