use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const EXPORT_SCHEMA_VERSION: u32 = 1;
pub const SOURCE_PACKAGE_APPIMAGE: &str = "appimage";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportManifest {
    pub schema_version: u32,
    pub deployd_version: String,
    pub source_package: String,
    pub exported_at: String,
    pub game_id: String,
    pub game_title: String,
    pub original_game_path: String,
    pub original_wine_prefix: Option<String>,
    pub advisory_downloads_dir: String,
    pub warnings: Vec<String>,
}

impl ExportManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != EXPORT_SCHEMA_VERSION {
            bail!("Unsupported export schema version: {}", self.schema_version);
        }
        if self.source_package != SOURCE_PACKAGE_APPIMAGE {
            bail!("Unsupported export source: {}", self.source_package);
        }
        if self.game_id.trim().is_empty() {
            bail!("Export manifest is missing a game ID");
        }
        if self.game_title.trim().is_empty() {
            bail!("Export manifest is missing a game title");
        }
        Ok(())
    }
}
