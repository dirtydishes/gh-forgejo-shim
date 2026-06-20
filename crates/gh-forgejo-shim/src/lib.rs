//! Rust workspace scaffold for `gh-forgejo-shim`.
//!
//! Phase 02 intentionally creates module boundaries and test harnesses without
//! porting command behavior. The installed Python package remains the runtime
//! until the cutover phase.

pub mod auth;
pub mod bootstrap;
pub mod cli;
pub mod codex_smoke;
pub mod config;
pub mod create;
pub mod doctor;
pub mod external;
pub mod forgejo;
pub mod git_recorder;
pub mod gui_path;
pub mod normalize;
pub mod repo;
pub mod routing;
pub mod shim;
pub mod trace;
pub mod trace_summary;

use std::fmt;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub type Result<T> = std::result::Result<T, ShimError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShimError {
    message: String,
}

impl ShimError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ShimError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ShimError {}
