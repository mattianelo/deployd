use crate::models::download::DownloadEntry;
use crate::models::game::Game;

use super::PreviewCounts;

#[derive(Debug, Clone)]
pub struct ImportBundleResult {
    pub game: Game,
    pub download_entries: Vec<DownloadEntry>,
    pub counts: PreviewCounts,
    pub warnings: Vec<String>,
}

pub(super) fn import_warnings(mut warnings: Vec<String>, tool_count: i64) -> Vec<String> {
    if tool_count > 0 {
        warnings.push(format!(
            "{tool_count} external tool(s) were skipped. Re-add tools in the Snap so they use the Snap Wine runtime."
        ));
    }
    warnings
}

pub(super) fn build_import_result(
    game: Game,
    download_entries: Vec<DownloadEntry>,
    counts: PreviewCounts,
    warnings: Vec<String>,
) -> ImportBundleResult {
    ImportBundleResult {
        game,
        download_entries,
        counts,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::import_warnings;

    #[test]
    fn preserves_manifest_warnings_without_skipped_tools() {
        let warnings = vec!["Export warning".to_owned()];

        assert_eq!(import_warnings(warnings.clone(), 0), warnings);
    }

    #[test]
    fn appends_skipped_tool_warning_after_manifest_warnings() {
        let warnings = import_warnings(vec!["Export warning".to_owned()], 2);

        assert_eq!(
            warnings,
            vec![
                "Export warning",
                "2 external tool(s) were skipped. Re-add tools in the Snap so they use the Snap Wine runtime."
            ]
        );
    }
}
