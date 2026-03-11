use reqwest::Client;

const NEXUS_API_BASE: &str = "https://api.nexusmods.com/v1";
pub const NEXUS_DOMAIN: &str = "site";
pub const NEXUS_MOD_ID: i64 = 174218;

const GITHUB_API_URL: &str =
    "https://api.github.com/repos/mattianelo/deployd/releases/latest";

pub const NEXUS_PAGE_URL: &str = "https://www.nexusmods.com/site/mods/174218";

pub struct ReleaseInfo {
    pub version: String,
    pub url: String,
}

/// Checks for a newer version of the app.
///
/// If `api_key` is provided, queries the Nexus Mods mod page (primary source).
/// Falls back to GitHub releases if Nexus is unavailable or no key is provided.
/// Errors are silently ignored so a network failure never affects the user.
pub async fn check_for_app_update(api_key: Option<String>) -> Option<ReleaseInfo> {
    if let Some(ref key) = api_key {
        if let Some(info) = check_via_nexus(key).await {
            return Some(info);
        }
    }
    check_via_github().await
}

async fn check_via_nexus(api_key: &str) -> Option<ReleaseInfo> {
    let client = Client::builder()
        .user_agent(concat!("deployd/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let resp = client
        .get(format!(
            "{NEXUS_API_BASE}/games/{NEXUS_DOMAIN}/mods/{NEXUS_MOD_ID}.json"
        ))
        .header("apikey", api_key)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;

    let json: serde_json::Value = resp.json().await.ok()?;
    let version = json["version"].as_str()?;

    if is_newer(version, env!("CARGO_PKG_VERSION")) {
        Some(ReleaseInfo {
            version: version.to_owned(),
            url: NEXUS_PAGE_URL.to_owned(),
        })
    } else {
        None
    }
}

async fn check_via_github() -> Option<ReleaseInfo> {
    let client = Client::builder()
        .user_agent(concat!("deployd/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let resp = client
        .get(GITHUB_API_URL)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;

    let json: serde_json::Value = resp.json().await.ok()?;
    let tag = json["tag_name"].as_str()?;
    let html_url = json["html_url"].as_str()?;

    // Strip leading 'v' so "v0.9.2" → "0.9.2"
    let remote_version = tag.trim_start_matches('v');

    if is_newer(remote_version, env!("CARGO_PKG_VERSION")) {
        Some(ReleaseInfo {
            version: remote_version.to_owned(),
            url: html_url.to_owned(),
        })
    } else {
        None
    }
}

/// Returns true if `remote` is strictly newer than `current`.
/// Compares three dot-separated numeric components (x.y.z).
fn is_newer(remote: &str, current: &str) -> bool {
    parse_semver(remote) > parse_semver(current)
}

fn parse_semver(v: &str) -> (u32, u32, u32) {
    let mut parts = v.splitn(3, '.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|s| s.split('-').next())   // strip pre-release suffix
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (major, minor, patch)
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn newer_patch() {
        assert!(is_newer("0.9.2", "0.9.1"));
    }

    #[test]
    fn same_version() {
        assert!(!is_newer("0.9.1", "0.9.1"));
    }

    #[test]
    fn older_version() {
        assert!(!is_newer("0.9.0", "0.9.1"));
    }

    #[test]
    fn newer_minor() {
        assert!(is_newer("0.10.0", "0.9.5"));
    }
}
