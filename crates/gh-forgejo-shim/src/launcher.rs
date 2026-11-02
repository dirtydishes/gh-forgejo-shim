//! The launcher: real gh against a Forgejo host through an in-process gateway on a private unix
//! socket. The design is `docs/rewrite/launcher.md`; section numbers below refer to it.

pub mod ghconfig;
#[cfg(unix)]
pub mod rundir;
