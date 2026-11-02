//! One launched call, start to finish (launcher spec §2, §4-§6): sweep, run directory, socket,
//! real gh as a child, the gateway on a current-thread runtime, signal forwarding, cleanup and
//! gh's exit status.

use std::ffi::OsString;
use std::fs::File;
use std::future::IntoFuture;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use rustix::process::{Pid, Signal};
use tokio::signal::unix::{signal, SignalKind};

use crate::external::inherited_exit_code;
use crate::launcher::gateway::{self, GatewayHost, PeerChecked};
use crate::launcher::rundir::{self, RunDir};

/// The text of each file §3 puts in the run directory.
#[derive(Debug, Clone, Default)]
pub struct Files {
    pub hosts_yml: String,
    pub credentials: String,
    pub gh_script: String,
    pub config_yml: String,
}

/// A process environment, as name and value pairs, byte for byte.
pub type Environment = Vec<(OsString, OsString)>;

/// Everything one launch needs. The closures run once the run directory exists, because the
/// files and gh's environment name its path.
pub struct Launch<'a> {
    /// Real gh's absolute path.
    pub gh: PathBuf,
    /// gh's arguments, after `gh`.
    pub args: Vec<OsString>,
    /// Where the run directory goes, already checked with [`rundir::check_parent`].
    pub parent: PathBuf,
    /// Every parent the sweep visits.
    pub sweep_parents: Vec<PathBuf>,
    pub gateway: Vec<GatewayHost>,
    pub files: Box<dyn FnOnce(&Path) -> Files + 'a>,
    /// gh's whole environment, given the run directory.
    pub env: Box<dyn FnOnce(&Path) -> Environment + 'a>,
}

/// Runs a launch and returns the code the wrapper exits with: gh's own, or 128 + s when gh died
/// of signal s, which `external::exit_like_child` then turns back into that death. Failures
/// before gh starts print one line and return §6's codes.
pub fn run(launch: Launch<'_>) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(1, &format!("could not start the gateway runtime: {error}")),
    };
    runtime.block_on(run_async(launch))
}

struct Signals {
    int: tokio::signal::unix::Signal,
    quit: tokio::signal::unix::Signal,
    term: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

impl Signals {
    /// Real handlers, never `SIG_IGN`: a caught signal reverts to the default across exec, so gh
    /// starts with its normal handling (§6).
    fn install() -> io::Result<Self> {
        Ok(Self {
            int: signal(SignalKind::interrupt())?,
            quit: signal(SignalKind::quit())?,
            term: signal(SignalKind::terminate())?,
            hup: signal(SignalKind::hangup())?,
        })
    }

    /// A signal that arrived since the handlers went in, if any. Polls each stream once.
    async fn pending(&mut self) -> Option<i32> {
        for (stream, number) in [
            (&mut self.int, Signal::INT),
            (&mut self.quit, Signal::QUIT),
            (&mut self.term, Signal::TERM),
            (&mut self.hup, Signal::HUP),
        ] {
            if tokio::time::timeout(Duration::ZERO, stream.recv())
                .await
                .is_ok()
            {
                return Some(number.as_raw());
            }
        }
        None
    }
}

async fn run_async(launch: Launch<'_>) -> i32 {
    let mut signals = match Signals::install() {
        Ok(signals) => signals,
        Err(error) => return fail(1, &format!("could not install signal handlers: {error}")),
    };
    rundir::sweep(&launch.sweep_parents);

    let run_dir = match RunDir::create(&launch.parent) {
        Ok(run_dir) => run_dir,
        Err(error) => {
            return fail(
                1,
                &format!(
                    "could not create a run directory in {}: {error}",
                    launch.parent.display()
                ),
            )
        }
    };
    let files = (launch.files)(run_dir.path());
    // §4's order: hosts.yml, credentials and the gh script, then config.yml, then the socket.
    let written = run_dir
        .write("hosts.yml", files.hosts_yml.as_bytes(), 0o600)
        .and_then(|_| run_dir.write("credentials", files.credentials.as_bytes(), 0o600))
        .and_then(|_| run_dir.write("gh", files.gh_script.as_bytes(), 0o700))
        .and_then(|_| run_dir.write("config.yml", files.config_yml.as_bytes(), 0o600));
    if let Err(error) = written {
        return fail(1, &format!("could not write the run directory: {error}"));
    }
    let router = match gateway::router(launch.gateway) {
        Ok(router) => router,
        Err(error) => return fail(1, &format!("could not set up the gateway: {error}")),
    };
    // Bound and listening before gh starts: an early connection waits in the backlog.
    let listener = match tokio::net::UnixListener::bind(run_dir.socket_path()) {
        Ok(listener) => listener,
        Err(error) => return fail(1, &format!("could not bind the gateway socket: {error}")),
    };
    let env = (launch.env)(run_dir.path());

    if let Some(number) = signals.pending().await {
        drop(listener);
        let _ = run_dir.remove();
        return die_by(number);
    }

    let mut child = match tokio::process::Command::new(&launch.gh)
        .args(&launch.args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let code = match error.kind() {
                io::ErrorKind::NotFound => 127,
                io::ErrorKind::PermissionDenied => 126,
                _ => 1,
            };
            return fail(
                code,
                &format!("could not run {}: {error}", launch.gh.display()),
            );
        }
    };
    let pid = child.id().and_then(|id| Pid::from_raw(id as i32));

    let serve = axum::serve(PeerChecked::new(listener), router).into_future();
    tokio::pin!(serve);
    let mut serving = true;
    let status: io::Result<ExitStatus> = loop {
        tokio::select! {
            status = child.wait() => break status,
            _ = signals.term.recv() => forward(pid, Signal::TERM),
            _ = signals.hup.recv() => forward(pid, Signal::HUP),
            _ = signals.int.recv() => if !in_terminal_foreground() { forward(pid, Signal::INT) },
            _ = signals.quit.recv() => if !in_terminal_foreground() { forward(pid, Signal::QUIT) },
            // The accept loop never ends on its own; if it somehow does, keep waiting for gh.
            _ = &mut serve, if serving => serving = false,
        }
    };
    // Dropping the serve future closes the listener and every in-flight connection: gh, their
    // only client, is gone.
    let _ = run_dir.remove();
    match status {
        Ok(status) => inherited_exit_code(status),
        Err(_) => 1,
    }
}

fn forward(pid: Option<Pid>, signal: Signal) {
    if let Some(pid) = pid {
        let _ = rustix::process::kill_process(pid, signal);
    }
}

/// A terminal sends Ctrl-C and Ctrl-\ to its whole foreground group, gh included, so forwarding
/// them would deliver them twice. Without a terminal, or in the background, the launcher is the
/// only one who got it (§6).
fn in_terminal_foreground() -> bool {
    let Ok(tty) = File::open("/dev/tty") else {
        return false;
    };
    rustix::termios::tcgetpgrp(&tty).is_ok_and(|group| group == rustix::process::getpgrp())
}

/// Ends the launch the way a signal before gh started would have: by that signal (§6).
fn die_by(number: i32) -> i32 {
    if std::process::id() != 1 {
        let _ = signal_hook::low_level::emulate_default_handler(number);
        let _ = signal_hook::low_level::raise(number);
    }
    128 + number
}

fn fail(code: i32, message: &str) -> i32 {
    eprintln!("gh-forgejo-shim: {message}");
    code
}
