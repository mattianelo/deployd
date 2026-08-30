pub(super) fn optional<T, E: std::fmt::Display>(
    result: Result<T, E>,
    description: &str,
    warnings: &mut Vec<String>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            warnings.push(format!("{description}: {error}"));
            None
        }
    }
}
