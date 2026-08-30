use std::path::Path;

pub(in crate::app) fn remove_mod_cache(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    std::fs::remove_dir_all(path)
        .err()
        .map(|error| format!("Could not remove mod cache '{}': {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::*;

    #[test]
    fn reports_cache_cleanup_failure() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let file = temp.path().join("cache-file");
        std::fs::write(&file, "cache")?;

        let warning = remove_mod_cache(&file);

        assert!(warning.is_some_and(|warning| warning.contains("Could not remove mod cache")));
        Ok(())
    }
}
