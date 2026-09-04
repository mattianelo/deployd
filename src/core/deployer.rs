mod application;
mod backup;
mod filesystem;
mod planning;
mod purge;
mod report;

#[cfg(test)]
mod tests;

pub use application::deploy;
pub(crate) use application::{deployment_preflight, vanilla_protection_enabled};
pub use purge::purge;
pub use report::{DeployOutcome, PurgeOutcome};
pub(crate) use report::{DeploymentPreflight, VanillaReplacementStatus};
