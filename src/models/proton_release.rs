/// A ProtonGE release fetched from the GitHub releases API.
#[derive(Debug, Clone)]
pub struct ProtonRelease {
    /// Version tag, e.g. `"GE-Proton9-20"`.
    pub tag: String,
    /// Direct URL to the `.tar.gz` tarball asset.
    pub download_url: String,
    /// `true` when this version is already extracted under `runtimes/<tag>/`.
    pub installed: bool,
}
