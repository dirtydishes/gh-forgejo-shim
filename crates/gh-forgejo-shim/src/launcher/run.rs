//! One launched call, start to finish (launcher spec §2, §4-§6): sweep, run directory, socket,
//! real gh as a child, the gateway on a current-thread runtime, signal forwarding, cleanup and
//! gh's exit status.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::future::IntoFuture;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};

use rustix::process::{Pid, Signal};
use tokio::signal::unix::{signal, SignalKind};

use crate::external::inherited_exit_code;
use crate::launcher::env::{self, Edit};
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

/// The variables a launch changes in gh's environment (§7), by name.
pub type Edits = Vec<(&'static str, Edit)>;

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
    /// The environment edits for gh, given the run directory.
    pub edits: Box<dyn FnOnce(&Path) -> Edits + 'a>,
}

const SIGNALS: [Signal; 4] = [Signal::INT, Signal::QUIT, Signal::TERM, Signal::HUP];

/// Runs a launch and returns the code the wrapper exits with: gh's own, or 128 + s when gh died
/// of signal s, which `external::exit_like_child` then turns back into that death. Failures
/// before gh starts print one line and return §6's codes.
///
/// The order matters. gh inherits this process's environment untouched, so the launcher edits
/// it in place (§7), which is sound only while no other thread can read it. So the signal
/// handlers, the run directory, its files and the edits all come before the gateway's runtime,
/// or any other thread, starts.
pub fn run(launch: Launch<'_>) -> i32 {
    install_panic_hook();
    // §6: real handlers before the run directory exists, never `SIG_IGN`, so gh starts with the
    // default actions. A signal caught before gh starts ends the launch by that signal.
    let caught = Arc::new(AtomicUsize::new(0));
    for signal in SIGNALS {
        let number = signal.as_raw();
        if let Err(error) =
            signal_hook::flag::register_usize(number, Arc::clone(&caught), number as usize)
        {
            return fail(1, &format!("could not install signal handlers: {error}"));
        }
    }
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
    // Dropped before `run_dir`, on every path out of here.
    let _panics = PanicFile::set(run_dir.path().join("panic"));
    let files = (launch.files)(run_dir.path());
    // §4's order: hosts.yml, credentials and the gh script, then config.yml, then the socket.
    // An empty `credentials` or `gh` isn't written: nothing would read it.
    let optional = |name, text: &str, mode| {
        if text.is_empty() {
            Ok(())
        } else {
            run_dir.write(name, text.as_bytes(), mode).map(drop)
        }
    };
    let written = run_dir
        .write("hosts.yml", files.hosts_yml.as_bytes(), 0o600)
        .and_then(|_| optional("credentials", &files.credentials, 0o600))
        .and_then(|_| optional("gh", &files.gh_script, 0o700))
        .and_then(|_| run_dir.write("config.yml", files.config_yml.as_bytes(), 0o600));
    if let Err(error) = written {
        return fail(1, &format!("could not write the run directory: {error}"));
    }

    let edits = (launch.edits)(run_dir.path());
    assert_single_threaded();
    env::apply(&edits);
    std::env::set_var(env::LAUNCHER, std::process::id().to_string());

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(1, &format!("could not start the gateway runtime: {error}")),
    };
    let outcome = runtime.block_on(serve_gh(
        &launch.gh,
        &launch.args,
        launch.gateway,
        &run_dir,
        &caught,
    ));
    // Dropping the runtime would wait for blocking tasks, such as a gateway request's DNS lookup
    // that gh no longer needs; the launcher's exit mustn't wait on them.
    runtime.shutdown_background();
    let _ = run_dir.remove();
    match outcome {
        Outcome::Code(code) => code,
        Outcome::Signalled(number) => die_by(number),
    }
}

enum Outcome {
    /// gh's status, or a failure code from before gh started.
    Code(i32),
    /// A signal arrived before gh started.
    Signalled(i32),
}

/// The environment edits are sound only while this is the process's only thread. On Linux,
/// debug builds check it.
fn assert_single_threaded() {
    #[cfg(all(debug_assertions, target_os = "linux"))]
    if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
        let threads = tasks.count();
        debug_assert_eq!(
            threads, 1,
            "the launcher edits its environment with {threads} threads"
        );
    }
}

async fn serve_gh(
    gh: &Path,
    args: &[OsString],
    gateway_hosts: Vec<GatewayHost>,
    run_dir: &RunDir,
    caught: &AtomicUsize,
) -> Outcome {
    let mut signals = match Signals::install() {
        Ok(signals) => signals,
        Err(error) => {
            return Outcome::Code(fail(
                1,
                &format!("could not install signal handlers: {error}"),
            ))
        }
    };
    let router = match gateway::router(gateway_hosts) {
        Ok(router) => router,
        Err(error) => {
            return Outcome::Code(fail(1, &format!("could not set up the gateway: {error}")))
        }
    };
    // Bound and listening before gh starts: an early connection waits in the backlog.
    let listener = match tokio::net::UnixListener::bind(run_dir.socket_path()) {
        Ok(listener) => listener,
        Err(error) => {
            return Outcome::Code(fail(
                1,
                &format!("could not bind the gateway socket: {error}"),
            ))
        }
    };
    let number = caught.load(Ordering::SeqCst);
    if number != 0 {
        return Outcome::Signalled(number as i32);
    }

    // No environment call: gh inherits this process's environment, edits included, byte for
    // byte (§7).
    let mut child = match tokio::process::Command::new(gh)
        .args(args)
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
            return Outcome::Code(fail(
                code,
                &format!("could not run {}: {error}", gh.display()),
            ));
        }
    };
    let pid = child.id().and_then(|id| Pid::from_raw(id as i32));
    // A signal between the check above and the spawn reaches its stream below and is handled
    // like any other. One case is lost: a terminal's Ctrl-C or Ctrl-\ in that window, which gh
    // missed and the terminal rule doesn't forward. Pressing it again works. Forwarding from the
    // flag instead can't tell that window from the moment after the spawn, when the terminal
    // already reached gh, so it would duplicate or swallow deliveries (spec §6).

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
    // Returning drops the serve future, which closes the listener and every in-flight
    // connection: gh, their only client, is gone.
    Outcome::Code(match status {
        Ok(status) => inherited_exit_code(status),
        Err(_) => 1,
    })
}

/// The streams the run loop forwards from, once gh runs. They sit beside the flags in `run`,
/// which caught anything that arrived earlier.
struct Signals {
    int: tokio::signal::unix::Signal,
    quit: tokio::signal::unix::Signal,
    term: tokio::signal::unix::Signal,
    hup: tokio::signal::unix::Signal,
}

impl Signals {
    fn install() -> io::Result<Self> {
        Ok(Self {
            int: signal(SignalKind::interrupt())?,
            quit: signal(SignalKind::quit())?,
            term: signal(SignalKind::terminate())?,
            hup: signal(SignalKind::hangup())?,
        })
    }
}

/// Where a panic goes while a run directory exists (§5).
static PANIC_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sends panics to a run directory's `panic` file until dropped.
struct PanicFile;

impl PanicFile {
    fn set(path: PathBuf) -> Self {
        if let Ok(mut file) = PANIC_FILE.lock() {
            *file = Some(path);
        }
        Self
    }
}

impl Drop for PanicFile {
    fn drop(&mut self) {
        if let Ok(mut file) = PANIC_FILE.lock() {
            *file = None;
        }
    }
}

/// A panic in a gateway task would otherwise print to stderr, which gh owns and clients parse.
/// While a run directory exists, the message goes to its `panic` file, removed with it; any
/// other time, the previous hook runs as before. Tracing it is a follow-up (trace.rs).
fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let path = PANIC_FILE.lock().ok().and_then(|file| file.clone());
            match path {
                Some(path) => {
                    let _ = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .mode(0o600)
                        .open(path)
                        .and_then(|mut file| writeln!(file, "{info}"));
                }
                None => previous(info),
            }
        }));
    });
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
