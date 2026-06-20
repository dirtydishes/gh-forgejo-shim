//! Command-line scaffold shared by the Rust binaries.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};

use crate::routing;
use crate::VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryName {
    GhForgejoShim,
    Gfj,
}

impl BinaryName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GhForgejoShim => "gh-forgejo-shim",
            Self::Gfj => "gfj",
        }
    }
}

pub fn run_from_env(binary: BinaryName) -> i32 {
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if binary == BinaryName::GhForgejoShim
        && args.first().is_some_and(|arg| arg == OsStr::new("gh"))
    {
        args.remove(0);
        return routing::run_gh_from_env(args);
    }

    let stdout = io::stdout();
    let stderr = io::stderr();
    run(binary, args, &mut stdout.lock(), &mut stderr.lock())
}

pub fn run<I>(binary: BinaryName, args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    let result =
        if binary == BinaryName::GhForgejoShim && args.first().is_some_and(|arg| arg == "gh") {
            Ok(routing::run_gh_captured(
                args[1..].to_vec(),
                std::env::vars().collect(),
                std::env::current_dir().ok().as_deref(),
                stdout,
                stderr,
            ))
        } else if args.is_empty() || is_one_of(&args, &["--help", "-h", "help"]) {
            print_help(binary, stdout)
        } else if is_one_of(&args, &["--version", "-V", "version"]) {
            print_version(stdout)
        } else {
            print_scaffold_only(binary, stderr)
        };

    result.unwrap_or(1)
}

fn is_one_of(args: &[String], values: &[&str]) -> bool {
    args.len() == 1 && values.contains(&args[0].as_str())
}

fn print_help(binary: BinaryName, stdout: &mut dyn Write) -> io::Result<i32> {
    let name = binary.as_str();
    writeln!(stdout, "{name} {VERSION}")?;
    writeln!(stdout)?;
    writeln!(stdout, "Rust scaffold for the gh-forgejo-shim rewrite.")?;
    writeln!(stdout)?;
    writeln!(stdout, "USAGE:")?;
    writeln!(stdout, "    {name} [--help]")?;
    writeln!(stdout, "    {name} [--version]")?;
    writeln!(stdout, "    {name} version")?;
    writeln!(stdout)?;
    writeln!(stdout, "STATUS:")?;
    writeln!(
        stdout,
        "    Rust rewrite in progress; managed gh dispatch is native, while management commands are ported by later phases."
    )?;
    writeln!(stdout)?;
    writeln!(stdout, "COMMANDS:")?;
    writeln!(stdout, "    help       print help")?;
    writeln!(stdout, "    version    print version")?;
    Ok(0)
}

fn print_version(stdout: &mut dyn Write) -> io::Result<i32> {
    writeln!(stdout, "{VERSION}")?;
    Ok(0)
}

fn print_scaffold_only(binary: BinaryName, stderr: &mut dyn Write) -> io::Result<i32> {
    writeln!(
        stderr,
        "{}: Rust command behavior is not implemented for this surface yet.",
        binary.as_str()
    )?;
    writeln!(
        stderr,
        "Only the managed gh dispatcher is native in this phase."
    )?;
    Ok(2)
}
