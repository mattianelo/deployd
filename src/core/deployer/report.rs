#[derive(Debug)]
pub struct DeployResult {
    pub files_total: usize,
    pub files_added: usize,
    pub files_removed: usize,
    pub conflicts_resolved: usize,
}
