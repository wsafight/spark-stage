pub mod adapter;
pub mod benchmark;
pub mod build;
pub mod cli;
pub mod domain;
pub mod evaluation;
pub mod ipc;
pub mod media;
pub mod notifications;
pub mod paths;
pub mod portability;
pub mod preflight;
pub mod store;
pub mod tui;
pub mod validation;
pub mod worker;

#[cfg(test)]
pub(crate) mod test_support;
