use reqwest::Client;

const NEXUS_API_BASE: &str = "https://api.nexusmods.com/v1";
pub const NEXUS_DOMAIN: &str = "skyrimspecialedition";
pub const NEXUS_MOD_ID: i64 = 174218;

pub const NEXUS_PAGE_URL: &str = "https://www.nexusmods.com/skyrimspecialedition/mods/174218";

pub struct ReleaseInfo {
    pub version: String,
    pub url: String,
}

/// Checks for a newer version of the app via the Nexus Mods API.
///
/// Returns `None` if no API key is provided, the request fails, or no newer
/// version is available. Errors are silently ignored so a network failure
/// never affects the user.
pub async fn check_for_app_update(api_key: Option<String>) -> Option<ReleaseInfo> {
    let key = api_key?;
    check_via_nexus(&key).await
}

async fn check_via_nexus(api_key: &str) -> Option<ReleaseInfo> {
    let client = Client::builder()
        .user_agent(concat!("deployd/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    check_via_nexus_with(&client, api_key, NEXUS_API_BASE, env!("CARGO_PKG_VERSION")).await
}

async fn check_via_nexus_with(
    client: &Client,
    api_key: &str,
    api_base: &str,
    current_version: &str,
) -> Option<ReleaseInfo> {
    let resp = client
        .get(format!(
            "{}/games/{NEXUS_DOMAIN}/mods/{NEXUS_MOD_ID}.json",
            api_base.trim_end_matches('/')
        ))
        .header("apikey", api_key)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;

    let json: serde_json::Value = resp.json().await.ok()?;
    let version = json["version"].as_str()?;

    if is_newer(version, current_version) {
        Some(ReleaseInfo {
            version: version.to_owned(),
            url: NEXUS_PAGE_URL.to_owned(),
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
        .and_then(|s| s.split('-').next()) // strip pre-release suffix
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (major, minor, patch)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    fn serve_once(status: &str, body: &str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind update test server");
        listener
            .set_nonblocking(true)
            .expect("make update test server nonblocking");
        let address = listener.local_addr().expect("read update test address");
        let status = status.to_string();
        let body = body.to_string();
        let (request_sender, request_receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while let Ok(read) = stream.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = request_sender.send(String::from_utf8_lossy(&request).into_owned());
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });

        (format!("http://{address}/v1"), request_receiver)
    }

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

    // @variants: both
    #[tokio::test]
    async fn update_request_uses_injected_endpoint_and_api_key() {
        let (api_base, request) = serve_once("200 OK", r#"{"version":"9.0.0"}"#);
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("build update test client");

        let release = check_via_nexus_with(&client, "test-key", &api_base, "1.0.0")
            .await
            .expect("newer release should be returned");
        let request = request
            .recv_timeout(Duration::from_secs(2))
            .expect("capture update request");

        assert_eq!(release.version, "9.0.0");
        assert_eq!(release.url, NEXUS_PAGE_URL);
        assert!(request.starts_with(&format!(
            "GET /v1/games/{NEXUS_DOMAIN}/mods/{NEXUS_MOD_ID}.json HTTP/1.1"
        )));
        assert!(request.to_ascii_lowercase().contains("apikey: test-key"));
    }

    // @variants: both
    #[tokio::test]
    async fn update_request_treats_http_failure_as_no_update() {
        let (api_base, _) = serve_once("503 Service Unavailable", "{}");
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("build update test client");

        assert!(
            check_via_nexus_with(&client, "test-key", &api_base, "1.0.0")
                .await
                .is_none()
        );
    }
}
