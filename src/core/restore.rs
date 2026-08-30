use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

pub(crate) fn apply_staged_database_restore(
    database: &Path,
    pending: &Path,
    marker: &Path,
) -> Result<bool> {
    if !pending.exists() {
        return Ok(false);
    }
    fs::rename(pending, database).with_context(|| {
        format!(
            "Failed to apply pending restore '{}' to '{}'",
            pending.display(),
            database.display()
        )
    })?;
    fs::File::create(marker)
        .with_context(|| format!("Failed to write restore marker '{}'", marker.display()))?;
    Ok(true)
}

pub(crate) fn consume_restore_marker(marker: &Path) -> (bool, Option<String>) {
    if !marker.exists() {
        return (false, None);
    }
    let warning = fs::remove_file(marker).err().map(|error| {
        format!(
            "The restore marker '{}' could not be cleared and may be shown again: {error}",
            marker.display()
        )
    });
    (true, warning)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::tempdir;

    use super::apply_staged_database_restore;

    #[test]
    fn reports_restore_marker_write_failure() -> Result<()> {
        let temp = tempdir()?;
        let database = temp.path().join("deployd.db");
        let pending = temp.path().join("pending.db");
        let marker = temp.path().join("missing").join("restored");
        std::fs::write(&pending, b"restored")?;

        let error = apply_staged_database_restore(&database, &pending, &marker)
            .expect_err("marker creation is a required restore operation");

        assert!(error.to_string().contains("restore marker"));
        assert_eq!(std::fs::read(database)?, b"restored");
        Ok(())
    }
}
