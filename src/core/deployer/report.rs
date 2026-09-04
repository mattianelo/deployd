#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VanillaReplacementStatus {
    ReadyToBackUp,
    Protected,
    BackupUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VanillaReplacement {
    pub(crate) path: String,
    pub(crate) status: VanillaReplacementStatus,
}

#[derive(Debug)]
pub(crate) struct DeploymentPreflight {
    pub(crate) protect_vanilla_files: bool,
    pub(crate) vanilla_replacements: Vec<VanillaReplacement>,
}

#[derive(Debug)]
pub struct DeployOutcome {
    pub files_total: usize,
    pub files_added: usize,
    pub files_removed: usize,
    pub conflicts_resolved: usize,
    pub vanilla_files_backed_up: usize,
    pub vanilla_files_restored: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct PurgeOutcome {
    pub files_removed: usize,
    pub vanilla_files_restored: usize,
    pub warnings: Vec<String>,
}
