//! Stateless Forgejo client and GitHub projections shared by HTTP and CLI adapters.
//! Nothing here writes to stdout or stderr: a launched gh owns both.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]
pub mod forgejo;
pub mod graphql;
pub mod ids;
pub mod map;
pub mod scopes;

pub mod actions;
pub mod search;
