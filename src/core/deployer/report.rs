#[derive(Debug)]
pub struct DeployOutcome {
    pub files_total: usize,
    pub files_added: usize,
    pub files_removed: usize,
    pub conflicts_resolved: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct PurgeOutcome {
    pub files_removed: usize,
    pub warnings: Vec<String>,
}
