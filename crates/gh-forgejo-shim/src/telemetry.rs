//! The gh wrapper's anonymous daily usage report (docs/telemetry.md).
//!
//! [`daily`] runs on every wrapper call and costs one file check. Once a day it starts
//! `gfj telemetry send` detached, in its own process group, with stdin, stdout and stderr closed,
//! and returns at once without waiting. That process builds the report and posts it within
//! [`usage::TIMEOUT`]. Nothing here prints during a gh call, and every failure stays silent.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use fga_server::usage::{self, Report, Tally};

use crate::config::{self, EnvMap};

/// The wrapper's off switch; `DO_NOT_TRACK` and `CI` count too.
pub const SWITCH: &str = "GFJ_TELEMETRY";
/// Another address for reports, for testing a collector. A build the release script didn't make
/// sends only here.
pub const URL_ENV: &str = "GFJ_TELEMETRY_URL";

/// What the notice says, once, from `bootstrap`, `install-shim` or `doctor`.
pub const NOTICE: &str =
    "gh-forgejo-shim has one maintainer, who can't test it with every agent. To find what breaks, \
it sends one anonymous usage report a day: its version, OS, which agent ran gh, the forge's \
software, and which GitHub features were missing or failed. No hosts, repositories, commands or \
IDs. Turn it off with `gfj telemetry off` or GFJ_TELEMETRY=0, and see the report with \
`gfj telemetry show`. Details: https://github.com/dirtydishes/gh-forgejo-shim/blob/main/docs/telemetry.md";

/// A variable's value, from whichever environment the caller holds.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    On(String),
    Off(String),
}

/// Whether this install sends reports, and where to or why not.
pub fn status(env: Env) -> Status {
    if let Some(reason) = usage::off_by_env(env, SWITCH) {
        return Status::Off(reason);
    }
    if config::telemetry_off(&config_path(env)) {
        return Status::Off("turned off with `gfj telemetry off`".to_string());
    }
    match usage::endpoint(env(URL_ENV)) {
        Some(url) => Status::On(url),
        None => Status::Off(format!(
            "this build wasn't made by the release script, so it sends reports only to {URL_ENV}"
        )),
    }
}

fn config_path(env: Env) -> PathBuf {
    config::config_path(env("HOME").map(PathBuf::from).as_deref())
}

/// `$XDG_STATE_HOME/gh-forgejo-shim/telemetry`, or `~/.local/state/…` without it.
fn state_dir(env: Env) -> Option<PathBuf> {
    let base = env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".local/state"))
        })?;
    Some(base.join("gh-forgejo-shim/telemetry"))
}

/// Deletes everything kept for reports: once they're off, nothing stays.
pub fn forget(env: Env) {
    if let Some(dir) = state_dir(env) {
        if fs::symlink_metadata(&dir).is_ok() {
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

/// Opens a state file without following a link or waiting on a pipe, and only if it's a regular
/// file, so no file put there can hold up the process. `append` creates it if absent.
fn open_state(path: &Path, append: bool) -> Option<fs::File> {
    let mut options = OpenOptions::new();
    if append {
        options.create(true).append(true);
    } else {
        options.read(true);
    }
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        (rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW).bits() as i32,
    );
    let file = options.open(path).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

fn read_state(path: &Path) -> Option<String> {
    let mut text = String::new();
    io::Read::read_to_string(&mut open_state(path, false)?, &mut text).ok()?;
    Some(text)
}

/// The wrapper's hook, before gh runs: on the first call of a UTC day, starts the sender. Cheap
/// on every other call: the switches, the config's setting, then one check for today's marker.
/// While reports are off, it deletes whatever was kept for them, today's marker too, which also
/// stops a sender already running.
pub fn daily(env: Env) {
    if usage::off_by_env(env, SWITCH).is_some() {
        forget(env);
        return;
    }
    if config::telemetry_off(&config_path(env)) {
        forget(env);
        return;
    }
    if usage::endpoint(env(URL_ENV)).is_none() {
        return;
    }
    let Some(dir) = state_dir(env) else {
        return;
    };
    let marker = dir.join(format!("sent-{}", usage::utc_day(usage::now())));
    if fs::symlink_metadata(&marker).is_ok() {
        return;
    }
    // Created only if absent, so of several calls at once just one starts a sender. It holds a
    // ticket only that sender knows, so a marker made again after an opt-out doesn't revive it.
    let ticket = ticket();
    let created = crate::shim::create_dirs(&dir, 0o700).is_ok()
        && OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
            .and_then(|mut file| file.write_all(ticket.as_bytes()))
            .is_ok();
    if !created {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(exe);
    command
        .args([
            "telemetry",
            "send",
            "--caller",
            &caller_pid().to_string(),
            "--ticket",
            &ticket,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Its own process group, so a Ctrl-C meant for gh doesn't reach it, and it never holds gh's
    // terminal. Never waited for: it outlives the call if it must, then init reaps it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let _ = command.spawn();
}

/// A value no other call will write to a marker: this process's ID and the time. It stays on this
/// machine, in the marker and the sender's arguments.
fn ticket() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// The process that ran this one: for the wrapper, the agent's shell or the agent itself.
fn caller_pid() -> u32 {
    #[cfg(unix)]
    {
        rustix::process::getppid().map_or(0, |pid| pid.as_raw_nonzero().get() as u32)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// `gfj telemetry send`: builds today's report and posts it. Silent whatever happens.
///
/// Reports are checked again after the report is built, which can take a few seconds, and only
/// then is anything written or sent: a `gfj telemetry off` meanwhile wins, and so does an opt-out
/// in another call's environment, which deletes today's marker ([`daily`]). The marker must still
/// hold this sender's `ticket`: one made again since belongs to another sender.
pub fn send(env: &EnvMap, caller: Option<u32>, ticket: &str) {
    let get = |name: &str| env.get(name).cloned();
    if !matches!(status(&get), Status::On(_)) {
        forget(&get);
        return;
    }
    // [`ticket`]'s form only, since it names a file.
    if ticket.is_empty() || !ticket.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return;
    }
    // First, while the processes that ran gh are most likely still there.
    let client = caller.and_then(detect_client).unwrap_or("other");
    let dir = state_dir(&get);
    // The marker of the day this started, which an opt-out elsewhere deletes.
    let marker = dir
        .as_ref()
        .map(|dir| dir.join(format!("sent-{}", usage::utc_day(usage::now()))));
    // Only while today's marker is still this sender's; checked again below, since an opt-out and
    // a new sender may come between.
    let owns = |marker: &Option<PathBuf>| {
        marker
            .as_deref()
            .and_then(read_state)
            .is_some_and(|held| held == ticket)
    };
    if !owns(&marker) {
        return;
    }
    // Moved aside, under this sender's own name, so counts kept from now on go to the next report
    // and no other sender's cleanup can touch them.
    let taken = dir.as_deref().and_then(|dir| {
        // 0.4.0's counts, which don't say which agent ran gh: a report can't carry them.
        let _ = fs::remove_file(dir.join("unsupported"));
        let taken = dir.join(format!("counts.sending-{ticket}"));
        fs::rename(dir.join(COUNTS), &taken).ok().map(|()| taken)
    });
    let report = build(env, client, dir.as_deref(), taken.as_deref());
    let cancelled = !owns(&marker);
    let Status::On(url) = status(&get) else {
        forget(&get);
        return;
    };
    // Stopped: the opt-out that deleted the marker deleted the rest, and a marker made since
    // belongs to another sender. Counts this one took were kept after that opt-out, so they go
    // back for the next report; if reports are off again, the next call deletes them.
    if cancelled {
        if let (Some(dir), Some(taken)) = (&dir, &taken) {
            if let Some(text) = read_state(taken) {
                if let Some(mut file) = open_state(&dir.join(COUNTS), true) {
                    let _ = file.write_all(text.as_bytes());
                }
            }
            let _ = fs::remove_file(taken);
        }
        return;
    }
    if let Some(taken) = &taken {
        let _ = fs::remove_file(taken);
    }
    if let Some(dir) = &dir {
        // Written once, the first time a report goes.
        if let Ok(mut file) = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("first-week"))
        {
            let _ = file.write_all(report.first_week.as_bytes());
        }
    }
    if let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(usage::TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        let _ = client.post(url).json(&report).send();
    }
    // Keep today's marker only, and none of the counts a 0.4.0 sender stopped part way through.
    if let Some(dir) = state_dir(&get) {
        let today = format!("sent-{}", usage::utc_day(usage::now()));
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with("sent-") && name != today)
                || name.starts_with("unsupported.sending-")
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// The report for `client`, from `dir`'s first week and the counts kept in `counts`. It writes
/// nothing.
fn build(env: &EnvMap, client: &'static str, dir: Option<&Path>, counts: Option<&Path>) -> Report {
    let get = |name: &str| env.get(name).cloned();
    let config = config::load_config_with_env(Some(&config_path(&get)), env).ok();
    let (forge, forge_major) = config
        .as_ref()
        .and_then(|config| forge(config, env))
        .unwrap_or((None, None));
    let (unsupported, failed) = counts.map(kept).unwrap_or_default();
    Report {
        schema: usage::SCHEMA,
        version: usage::report_version(crate::VERSION),
        os: usage::os(),
        arch: usage::arch(),
        mode: "client",
        client: Some(client),
        forge,
        forge_major,
        gh: config.as_ref().and_then(|config| gh_version(config, env)),
        first_week: dir
            .and_then(|dir| read_state(&dir.join("first-week")))
            .map(|week| week.trim().to_string())
            .filter(|week| usage::is_week(week))
            .unwrap_or_else(|| usage::iso_week(usage::now())),
        unsupported: Tally::ByClient(unsupported),
        failed: Tally::ByClient(failed),
    }
    .bounded()
}

/// The first allowlisted forge's software, asked without a token: Forgejo answers its own version
/// route, and Forgejo and Gitea both answer `/api/v1/version`.
fn forge(config: &config::Config, env: &EnvMap) -> Option<(Option<&'static str>, Option<u64>)> {
    let host = config.hosts.first()?;
    let client = crate::forgejo::ForgejoClient::for_host(host, None, config, env)
        .ok()?
        .with_timeout(Duration::from_secs(1));
    let forgejo_route = client.version_at(host, "/api/forgejo/v1/version");
    let api = match forgejo_route {
        Some(_) => None,
        None => client.version_at(host, "/api/v1/version"),
    };
    Some(usage::forge(forgejo_route.as_deref(), api.as_deref()))
}

/// Real gh's `major.minor`.
fn gh_version(config: &config::Config, env: &EnvMap) -> Option<String> {
    let fallback_dirs = crate::external::DEFAULT_FALLBACK_DIRS
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let gh = crate::setup::find_tool(
        "gh",
        config.paths.gh.as_deref().map(Path::new),
        env,
        &fallback_dirs,
        true,
    )?;
    match crate::gh_version::probe(&gh.path, env) {
        crate::gh_version::Reading::Release { version, .. }
        | crate::gh_version::Reading::Dev {
            after: Some(version),
            ..
        } => Some(format!("{}.{}", version.major, version.minor)),
        _ => None,
    }
}

/// `gfj telemetry show`: today's report as [`send`] would post it, except that `client` names what
/// ran `show`, where the day's report names what ran gh.
pub fn show(env: &EnvMap) -> String {
    let get = |name: &str| env.get(name).cloned();
    let dir = state_dir(&get);
    let client = detect_client(caller_pid()).unwrap_or("other");
    let kept = dir.as_ref().map(|dir| dir.join(COUNTS));
    serde_json::to_string(&build(env, client, dir.as_deref(), kept.as_deref())).unwrap_or_default()
}

/// Where counts wait for the next report, a line each: `<kind> <client> <name> <count>`.
const COUNTS: &str = "counts";

/// Keeps what a launched call's gateway counted, under the agent that ran gh, for the next report.
/// It counts only while reports are on ([`usage::start_counting`]). Only a call with counts looks
/// for its agent, which reads the local process list.
pub fn keep_counts(env: Env) {
    let counts = usage::take_counts();
    if counts.is_empty() {
        return;
    }
    if !matches!(status(env), Status::On(_)) {
        forget(env);
        return;
    }
    let Some(dir) = state_dir(env) else {
        return;
    };
    let client = detect_client(caller_pid()).unwrap_or("other");
    let lines = [
        ("unsupported", &counts.unsupported),
        ("failed", &counts.failed),
    ]
    .into_iter()
    .flat_map(|(kind, names)| {
        names
            .iter()
            .map(move |(name, count)| format!("{kind} {client} {name} {count}\n"))
    })
    .collect::<String>();
    // One append-mode write, so calls at once each add whole lines.
    if crate::shim::create_dirs(&dir, 0o700).is_ok() {
        if let Some(mut file) = open_state(&dir.join(COUNTS), true) {
            let _ = file.write_all(lines.as_bytes());
        }
    }
}

/// Counts by client, then by name.
type ByClient = BTreeMap<&'static str, BTreeMap<String, u64>>;

/// The unsupported and failed counts in a file [`keep_counts`] wrote: known kinds and clients, and
/// vocabulary names, only. Any other line is passed over.
fn kept(path: &Path) -> (ByClient, ByClient) {
    let (mut unsupported, mut failed) = (ByClient::new(), ByClient::new());
    for line in read_state(path).unwrap_or_default().lines() {
        let mut fields = line.splitn(4, ' ');
        let (Some(kind), Some(client), Some(name), Some(count)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let by_client = match kind {
            "unsupported" => &mut unsupported,
            "failed" => &mut failed,
            _ => continue,
        };
        let (Some(client), Some(name), Ok(count)) = (
            usage::CLIENTS.into_iter().find(|known| *known == client),
            usage::known_name(name),
            count.parse::<u64>(),
        ) else {
            continue;
        };
        let total = by_client
            .entry(client)
            .or_default()
            .entry(name)
            .or_default();
        *total = total.saturating_add(count).min(usage::MAX_COUNT);
    }
    (unsupported, failed)
}

/// Prints the notice to `out` the first time one of the setup commands runs while reports are on.
pub fn notice(env: &EnvMap, out: &mut dyn Write) -> io::Result<()> {
    let get = |name: &str| env.get(name).cloned();
    if !matches!(status(&get), Status::On(_)) {
        return Ok(());
    }
    let Some(dir) = state_dir(&get) else {
        return Ok(());
    };
    let shown = crate::shim::create_dirs(&dir, 0o700).is_ok()
        && OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("notice-shown"))
            .is_ok();
    if shown {
        writeln!(out, "\n{NOTICE}")?;
    }
    Ok(())
}

/// The agent or app that ran gh, from the processes above `pid`: the nearest one this knows. Only
/// those processes are read, one at a time, so a call that keeps counts reads a few, not every one.
fn detect_client(pid: u32) -> Option<&'static str> {
    let mut pid = pid;
    for _ in 0..32 {
        let (parent, command) = process(pid)?;
        if let Some(client) = classify(&command) {
            return Some(client);
        }
        if parent <= 1 || parent == pid {
            return None;
        }
        pid = parent;
    }
    None
}

/// A process's parent and command line, read locally and never sent: from `/proc` where there is
/// one, else from `ps`.
fn process(pid: u32) -> Option<(u32, String)> {
    let dir = Path::new("/proc").join(pid.to_string());
    if let Ok(stat) = fs::read_to_string(dir.join("stat")) {
        // `pid (comm) state ppid …`; the name may hold spaces or parentheses.
        let parent = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|ppid| ppid.parse().ok())?;
        let command = fs::read(dir.join("cmdline"))
            .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
            .unwrap_or_default();
        return Some((parent, command));
    }
    if Path::new("/proc/self/stat").exists() {
        // Linux, and the process is gone.
        return None;
    }
    let output = Command::new("/bin/ps")
        .args(["-o", "ppid=", "-o", "args=", "-p", &pid.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.trim_start().splitn(2, char::is_whitespace);
    let parent = fields.next()?.parse().ok()?;
    Some((parent, fields.next().unwrap_or_default().trim().to_string()))
}

/// The app bundle a command line's program runs from, such as `/applications/codex.app/`: the
/// first `.app/` in its first word, the program's own path, never in an argument such as a shell's
/// `-c` text or an `env` setting. The agents' bundle names hold no spaces, so a helper's path with
/// spaces further on (`Cursor.app/…/Cursor Helper (Plugin).app/…`) still starts with its bundle.
fn bundle(lower: &str) -> Option<&str> {
    let program = lower.split_whitespace().next()?;
    Some(&program[..program.find(".app/")? + ".app/".len()])
}

/// Which of [`usage::CLIENTS`] a command line is, if any.
fn classify(command: &str) -> Option<&'static str> {
    let lower = command.to_ascii_lowercase();
    let mut words = lower.split_whitespace();
    let base = |word: &str| word.rsplit('/').next().unwrap_or(word).to_string();
    let program = base(words.next()?);
    let script_path = words.next().unwrap_or_default();
    let script = base(script_path);
    let app = bundle(&lower).unwrap_or_default();
    if app.ends_with("/chatgpt.app/") || program == "chatgpt" {
        return Some("chatgpt");
    }
    if app.ends_with("/claude.app/") {
        return Some("claude-desktop");
    }
    if app.ends_with("/cursor.app/") || program == "cursor" || program == "cursor-agent" {
        return Some("cursor");
    }
    if app.ends_with("/codex.app/") || program == "codex" || program.starts_with("codex-") {
        return Some("codex");
    }
    if program == "claude" {
        return Some("claude-code");
    }
    if matches!(program.as_str(), "node" | "bun" | "deno") {
        // Only the script the runtime runs, never a later argument such as an output path.
        if script == "claude" || script_path.contains("/claude-code/") {
            return Some("claude-code");
        }
        if script == "codex" || script == "codex.js" || script_path.contains("/@openai/codex/") {
            return Some("codex");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_names_its_client() {
        let cases = [
            ("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT", Some("chatgpt")),
            ("/Applications/Claude.app/Contents/MacOS/Claude", Some("claude-desktop")),
            (
                "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Plugin).app/Contents/MacOS/Cursor Helper (Plugin) --type=utility",
                Some("cursor"),
            ),
            ("/Applications/Codex.app/Contents/Resources/codex app-server", Some("codex")),
            ("/opt/homebrew/bin/codex exec", Some("codex")),
            ("node /usr/local/lib/node_modules/@openai/codex/bin/codex.js", Some("codex")),
            ("/Users/me/.local/bin/claude --resume", Some("claude-code")),
            ("node /usr/local/lib/node_modules/@anthropic-ai/claude-code/cli.js", Some("claude-code")),
            ("/bin/zsh -c gh pr view", None),
            // An app's path in an argument isn't the program's.
            ("/bin/zsh -c ls /Applications/Codex.app/Contents && gh pr view", None),
            ("/bin/sh /opt/x/ChatGPT.app/run gh pr view", None),
            ("/usr/bin/env ./Claude.app/Contents/MacOS/Claude", None),
            ("/usr/bin/env PATH=/Applications/Codex.app/Contents/Resources:/usr/bin bash -c gh", None),
            ("/bin/bash build/Codex.app/run.sh", None),
            ("/usr/bin/git clone https://example.com/Cursor.app/x", None),
            ("/bin/zsh x:/Claude.app/y", None),
            ("node ./Cursor.app/x.js", None),
            // A path in a later argument isn't the script's.
            ("node build.js /work/@openai/codex/output", None),
            ("node build.js /work/claude-code/output", None),
            ("/Applications/T3 Code (Nightly).app/Contents/MacOS/T3 Code --type=/codex.app/", None),
            (
                "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node /Applications/ChatGPT.app/Contents/Resources/x.mjs",
                Some("chatgpt"),
            ),
            ("/opt/x/Codex.app/bash /opt/x/run gh pr view", Some("codex")),
            ("node server.js", None),
            ("", None),
        ];
        for (command, client) in cases {
            assert_eq!(classify(command), client, "{command}");
        }
    }

    #[test]
    fn a_process_s_parent_and_command_line_are_known() {
        let (parent, command) = process(std::process::id()).expect("this process");
        assert_eq!(parent, caller_pid());
        assert!(command.contains("gh_forgejo_shim"), "{command}");
        assert_eq!(process(u32::MAX), None);
    }
}
