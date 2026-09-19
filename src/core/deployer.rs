mod application;
mod backup;
mod filesystem;
mod planning;
mod purge;
mod report;

#[cfg(test)]
mod tests;

pub(crate) use application::{deployment_preflight, vanilla_protection_enabled};
pub use report::{DeployOutcome, PurgeOutcome};
pub(crate) use report::{DeploymentPreflight, VanillaReplacementStatus};

pub(crate) use backup::backup_vanilla_file;
