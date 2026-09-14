mod bethesda;
mod catalog;
pub(crate) mod content;
mod coordinator;
mod divergence;
mod manifest;
mod mele;
mod operation;
mod ownership;
mod records;
mod store;
mod target;

mod history;
mod journal;
mod prepared;
mod recovery;
mod restore;
mod saves;
mod shared;
mod state;
#[cfg(test)]
mod tests;
mod validation;

#[cfg(test)]
mod state_tests;

#[cfg(test)]
#[path = "../../../tests/generations/generated.rs"]
mod generated_tests;

#[cfg(test)]
#[path = "../../../tests/generations/mele.rs"]
mod mele_tests;
