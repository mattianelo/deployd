use std::path::Path;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use reqwest::header::HeaderMap;
use tokio::io::AsyncWriteExt;

use crate::models::nexus::{DownloadLink, NexusFilesResponse, NexusModInfo, NexusUser};

const BASE_URL: &str = "https://api.nexusmods.com/v1";
const SSO_URL: &str = "wss://sso.nexusmods.com";
const SSO_BROWSER_URL: &str = "https://www.nexusmods.com/sso";
const APPLICATION_SLUG: &str = "mattianelo-deployd";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RateLimitInfo {
    pub hourly_remaining: u32,
    pub hourly_limit: u32,
    pub daily_remaining: u32,
    pub daily_limit: u32,
}

fn parse_rate_limits(headers: &HeaderMap) -> Option<RateLimitInfo> {
    let get = |name: &str| -> Option<u32> { headers.get(name)?.to_str().ok()?.parse().ok() };
    Some(RateLimitInfo {
        hourly_remaining: get("x-rl-hourly-remaining")?,
        hourly_limit: get("x-rl-hourly-limit")?,
        daily_remaining: get("x-rl-daily-remaining")?,
        daily_limit: get("x-rl-daily-limit")?,
    })
}

pub struct NexusClient {
    client: Client,
    api_key: String,
}

impl NexusClient {
    pub fn new(api_key: String) -> Self {
        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.insert(
            "Application-Name",
            reqwest::header::HeaderValue::from_static("Deployd"),
        );
        default_headers.insert(
            "Application-Version",
            reqwest::header::HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
        );
        let client = Client::builder()
            .default_headers(default_headers)
            .build()
            .expect("failed to build HTTP client");
        Self { client, api_key }
    }

    /// Validate the API key and return user info + rate limits.
    pub async fn validate_key(&self) -> Result<(NexusUser, Option<RateLimitInfo>)> {
        let resp = self
            .client
            .get(format!("{BASE_URL}/users/validate.json"))
            .header("apikey", &self.api_key)
            .send()
            .await
            .context("network error contacting Nexus API")?;

        if resp.status() == 401 {
            bail!("invalid API key");
        }
        if !resp.status().is_success() {
            bail!("Nexus API error: {}", resp.status());
        }

        let rate_limits = parse_rate_limits(resp.headers());
        let user = resp
            .json::<NexusUser>()
            .await
            .context("failed to parse Nexus user response")?;
        Ok((user, rate_limits))
    }

    /// Get mod details.
    pub async fn get_mod_info(
        &self,
        domain: &str,
        mod_id: i64,
    ) -> Result<(NexusModInfo, Option<RateLimitInfo>)> {
        let resp = self
            .client
            .get(format!("{BASE_URL}/games/{domain}/mods/{mod_id}.json"))
            .header("apikey", &self.api_key)
            .send()
            .await
            .context("network error fetching mod info")?;

        if !resp.status().is_success() {
            bail!("Nexus API error: {}", resp.status());
        }

        let rate_limits = parse_rate_limits(resp.headers());
        let info = resp
            .json::<NexusModInfo>()
            .await
            .context("failed to parse mod info response")?;
        Ok((info, rate_limits))
    }

    /// Get the file list for a mod.
    pub async fn get_mod_files(
        &self,
        domain: &str,
        mod_id: i64,
    ) -> Result<(NexusFilesResponse, Option<RateLimitInfo>)> {
        let resp = self
            .client
            .get(format!(
                "{BASE_URL}/games/{domain}/mods/{mod_id}/files.json"
            ))
            .header("apikey", &self.api_key)
            .send()
            .await
            .context("network error fetching mod files")?;

        if !resp.status().is_success() {
            bail!("Nexus API error: {}", resp.status());
        }

        let rate_limits = parse_rate_limits(resp.headers());
        let files = resp
            .json::<NexusFilesResponse>()
            .await
            .context("failed to parse mod files response")?;
        Ok((files, rate_limits))
    }

    /// Look up a file by its MD5 hash.
    ///
    /// Returns every `(mod, file_details)` pair on Nexus that matches the hash.
    /// In practice the list has exactly one entry.  An empty list means the hash
    /// is not indexed (very new upload, private file, or non-Nexus archive).
    pub async fn md5_search(
        &self,
        domain: &str,
        md5: &str,
    ) -> Result<(Vec<crate::models::nexus::Md5SearchResult>, Option<RateLimitInfo>)> {
        let resp = self
            .client
            .get(format!(
                "{BASE_URL}/games/{domain}/mods/md5_search/{md5}.json"
            ))
            .header("apikey", &self.api_key)
            .send()
            .await
            .context("network error during MD5 search")?;

        if !resp.status().is_success() {
            bail!("Nexus API error: {}", resp.status());
        }

        let rate_limits = parse_rate_limits(resp.headers());
        let results = resp
            .json::<Vec<crate::models::nexus::Md5SearchResult>>()
            .await
            .context("failed to parse MD5 search response")?;
        Ok((results, rate_limits))
    }

    /// Get download links for a specific file.
    ///
    /// For free users, `key` and `expires` must be provided (from the NXM link).
    /// Premium users can call without these parameters.
    pub async fn get_download_links(
        &self,
        domain: &str,
        mod_id: i64,
        file_id: i64,
        key: Option<&str>,
        expires: Option<&str>,
    ) -> Result<(Vec<DownloadLink>, Option<RateLimitInfo>)> {
        let mut url = reqwest::Url::parse(&format!(
            "{BASE_URL}/games/{domain}/mods/{mod_id}/files/{file_id}/download_link.json"
        ))
        .context("failed to construct download URL")?;

        if let (Some(k), Some(e)) = (key, expires) {
            url.query_pairs_mut().append_pair("key", k).append_pair("expires", e);
        }

        let resp = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .send()
            .await
            .context("network error fetching download link")?;

        if resp.status() == 403 {
            bail!("premium account required for direct downloads without a download key");
        }
        if !resp.status().is_success() {
            bail!("Nexus API error: {}", resp.status());
        }

        let rate_limits = parse_rate_limits(resp.headers());
        let links = resp
            .json::<Vec<DownloadLink>>()
            .await
            .context("failed to parse download links response")?;
        Ok((links, rate_limits))
    }

    /// Download a file from a URL to a local path, reporting progress.
    ///
    /// `on_progress` receives (bytes_downloaded, total_bytes). Total may be 0 if unknown.
    pub async fn download_file(
        &self,
        url: &str,
        dest: &Path,
        on_progress: impl Fn(u64, u64) + Send,
    ) -> Result<()> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .context("network error starting download")?;

        if !resp.status().is_success() {
            bail!("download failed: HTTP {}", resp.status());
        }

        let total = resp.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;
        // Throttle UI updates: fire at most once per 200 ms or every 0.5% of progress.
        // Without this, every chunk floods the Relm4 message queue and freezes the UI.
        let mut last_frac: f64 = -1.0;
        let mut last_tick = std::time::Instant::now();

        let mut file = tokio::fs::File::create(dest)
            .await
            .context("failed to create download file")?;

        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("error reading download stream")?;
            file.write_all(&chunk)
                .await
                .context("error writing download file")?;
            downloaded += chunk.len() as u64;

            let frac = if total > 0 {
                downloaded as f64 / total as f64
            } else {
                0.0
            };
            let now = std::time::Instant::now();
            if (frac - last_frac) >= 0.005 || now.duration_since(last_tick).as_millis() >= 200 {
                on_progress(downloaded, total);
                last_frac = frac;
                last_tick = now;
            }
        }

        // Always fire a final update so the progress bar reaches 100 %.
        on_progress(downloaded, total);

        file.flush().await?;
        Ok(())
    }
}

/// Perform Nexus Mods SSO login via WebSocket.
///
/// Opens the user's browser for authorization and waits for the API key
/// to be returned via the WebSocket connection.
pub async fn sso_login() -> Result<String> {
    use tokio_tungstenite::tungstenite::Message;

    let id = uuid::Uuid::new_v4().to_string();

    let (mut ws, _) = tokio_tungstenite::connect_async(SSO_URL)
        .await
        .context("failed to connect to Nexus SSO WebSocket")?;

    // Send handshake
    let handshake = serde_json::json!({
        "id": id,
        "token": null,
        "protocol": 2
    });
    ws.send(Message::Text(handshake.to_string()))
        .await
        .context("failed to send SSO handshake")?;

    // Wait for connection token acknowledgement
    let ack = ws
        .next()
        .await
        .context("SSO WebSocket closed before acknowledgement")?
        .context("SSO WebSocket error")?;

    let ack_text = ack.into_text().context("SSO ack is not text")?;
    let ack_json: serde_json::Value =
        serde_json::from_str(&ack_text).context("failed to parse SSO ack")?;

    if ack_json.get("success") != Some(&serde_json::Value::Bool(true)) {
        bail!(
            "SSO handshake failed: {}",
            ack_json
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown error")
        );
    }

    // Open browser for user authorization
    let browser_url = format!("{SSO_BROWSER_URL}?id={id}&application={APPLICATION_SLUG}");
    open::that(&browser_url).context("failed to open browser for Nexus login")?;

    // Wait for API key response
    while let Some(msg) = ws.next().await {
        let msg = msg.context("SSO WebSocket error while waiting for API key")?;
        let text = match msg {
            Message::Text(t) => t,
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => bail!("SSO WebSocket closed before receiving API key"),
            _ => continue,
        };

        let json: serde_json::Value =
            serde_json::from_str(&text).context("failed to parse SSO response")?;

        if json.get("success") == Some(&serde_json::Value::Bool(true))
            && let Some(api_key) = json.pointer("/data/api_key").and_then(|k| k.as_str())
        {
            return Ok(api_key.to_string());
        }

        if let Some(error) = json.get("error").and_then(|e| e.as_str())
            && !error.is_empty()
            && error != "null"
        {
            bail!("SSO login failed: {error}");
        }
    }

    bail!("SSO WebSocket closed without providing an API key")
}
