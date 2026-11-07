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

use fga_server::usage::{self, Report};

use crate::config::{self, EnvMap};

/// The wrapper's off switch; `DO_NOT_TRACK` and `CI` count too.
pub const SWITCH: &str = "GFJ_TELEMETRY";
/// Another address for reports, for testing a collector. A debug build sends only here.
pub const URL_ENV: &str = "GFJ_TELEMETRY_URL";

/// What the notice says, once, from `bootstrap`, `install-shim` or `doctor`.
pub const NOTICE: &str =
    "gh-forgejo-shim sends one anonymous usage report a day: its version, OS, \
which agent ran gh, and the forge's software. No hosts, repositories, commands or IDs. Turn it off \
with `gfj telemetry off` or GFJ_TELEMETRY=0, and see the exact report with `gfj telemetry show`. \
Details: https://github.com/dirtydishes/gh-forgejo-shim/blob/main/docs/telemetry.md";

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
    if config::telemetry_setting(&config_path(env)) == Some(false) {
        return Status::Off("turned off with `gfj telemetry off`".to_string());
    }
    match usage::endpoint(env(URL_ENV)) {
        Some(url) => Status::On(url),
        None => Status::Off(format!(
            "this is a debug build, which sends reports only to {URL_ENV}"
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

/// The wrapper's hook, before gh runs: on the first call of a UTC day, starts the sender. Cheap
/// on every other call: the switches, then one check for today's marker.
pub fn daily(env: Env) {
    if usage::off_by_env(env, SWITCH).is_some() || usage::endpoint(env(URL_ENV)).is_none() {
        return;
    }
    let Some(dir) = state_dir(env) else {
        return;
    };
    let marker = dir.join(format!("sent-{}", usage::utc_day(usage::now())));
    if marker.exists() || config::telemetry_setting(&config_path(env)) == Some(false) {
        return;
    }
    // Created only if absent, so of several calls at once just one starts a sender.
    if crate::shim::create_dirs(&dir, 0o700).is_err()
        || OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
            .is_err()
    {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(exe);
    command
        .args(["telemetry", "send", "--caller", &caller_pid().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Its own process group, so a Ctrl-C meant for gh doesn't reach it, and it never holds gh's
    // terminal. Never waited for: it outlives the call if it must, then init reaps it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let _ = command.spawn();
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
pub fn send(env: &EnvMap, caller: Option<u32>) {
    let get = |name: &str| env.get(name).cloned();
    let Status::On(url) = status(&get) else {
        return;
    };
    let report = build(env, caller, true);
    if let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(usage::TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        let _ = client.post(url).json(&report).send();
    }
    // Keep today's marker only.
    if let Some(dir) = state_dir(&get) {
        let today = format!("sent-{}", usage::utc_day(usage::now()));
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("sent-") && name != today {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// The report [`send`] posts. `sending` keeps it: it records the first week and takes the
/// unsupported counts; `gfj telemetry show` leaves both as they are.
pub fn build(env: &EnvMap, caller: Option<u32>, sending: bool) -> Report {
    let get = |name: &str| env.get(name).cloned();
    let dir = state_dir(&get);
    let config = config::load_config_with_env(Some(&config_path(&get)), env).ok();
    let (forge, forge_major) = config
        .as_ref()
        .and_then(|config| forge(config, env))
        .unwrap_or((None, None));
    Report {
        schema: usage::SCHEMA,
        version: crate::VERSION.to_string(),
        os: usage::os(),
        arch: usage::arch(),
        mode: "client",
        client: Some(caller.and_then(detect_client).unwrap_or("other")),
        forge,
        forge_major,
        gh: config.as_ref().and_then(|config| gh_version(config, env)),
        first_week: first_week(dir.as_deref(), sending),
        unsupported: dir
            .as_deref()
            .map(|dir| unsupported(dir, sending))
            .unwrap_or_default(),
    }
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

/// `gfj telemetry show`: today's report, byte for byte as [`send`] would post it.
pub fn show(env: &EnvMap) -> String {
    serde_json::to_string(&build(env, Some(caller_pid()), false)).unwrap_or_default()
}

/// The week of the first report, from `dir/first-week`, written there when `sending`.
fn first_week(dir: Option<&Path>, sending: bool) -> String {
    let this_week = usage::iso_week(usage::now());
    let Some(dir) = dir else {
        return this_week;
    };
    let path = dir.join("first-week");
    match fs::read_to_string(&path) {
        Ok(week) if usage::is_week(week.trim()) => week.trim().to_string(),
        _ => {
            if sending && crate::shim::create_dirs(dir, 0o700).is_ok() {
                let _ = fs::write(&path, &this_week);
            }
            this_week
        }
    }
}

/// Keeps the unsupported counts a launched call's gateway gathered, for the next report. Nothing
/// is kept while reports are off.
pub fn keep_unsupported(env: Env) {
    let counts = usage::take_unsupported();
    if counts.is_empty() || !matches!(status(env), Status::On(_)) {
        return;
    }
    let Some(dir) = state_dir(env) else {
        return;
    };
    let lines = counts
        .iter()
        .map(|(name, count)| format!("{name} {count}\n"))
        .collect::<String>();
    // One append-mode write, so calls at once each add whole lines.
    if crate::shim::create_dirs(&dir, 0o700).is_ok() {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("unsupported"))
        {
            let _ = file.write_all(lines.as_bytes());
        }
    }
}

/// The unsupported counts kept since the last report, vocabulary names only. When `sending`, they
/// move aside first, so counts kept meanwhile go to the next report.
fn unsupported(dir: &Path, sending: bool) -> BTreeMap<String, u64> {
    let kept = dir.join("unsupported");
    let path = if sending {
        let taken = dir.join("unsupported.sending");
        if fs::rename(&kept, &taken).is_err() {
            return BTreeMap::new();
        }
        taken
    } else {
        kept
    };
    let text = fs::read_to_string(&path).unwrap_or_default();
    if sending {
        let _ = fs::remove_file(&path);
    }
    let mut counts = BTreeMap::new();
    for line in text.lines() {
        let Some((name, count)) = line.split_once(' ') else {
            continue;
        };
        let (Some(name), Ok(count)) = (usage::known_name(name), count.parse::<u64>()) else {
            continue;
        };
        let total: &mut u64 = counts.entry(name).or_default();
        *total = total.saturating_add(count).min(usage::MAX_COUNT);
    }
    counts
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
    let marker = dir.join("notice-shown");
    if marker.exists()
        || crate::shim::create_dirs(&dir, 0o700).is_err()
        || fs::write(&marker, "").is_err()
    {
        return Ok(());
    }
    writeln!(out, "\n{NOTICE}")
}

/// The agent or app that ran gh, from the processes above `pid`: the nearest one this knows.
fn detect_client(pid: u32) -> Option<&'static str> {
    let table = processes();
    let mut pid = pid;
    for _ in 0..32 {
        let (parent, command) = table.get(&pid)?;
        if let Some(client) = classify(command) {
            return Some(client);
        }
        if *parent <= 1 || *parent == pid {
            return None;
        }
        pid = *parent;
    }
    None
}

/// Every process's parent and command line, read locally and never sent.
fn processes() -> BTreeMap<u32, (u32, String)> {
    let mut table = BTreeMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let stat = fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
            // `pid (comm) state ppid …`; the name may hold spaces or parentheses.
            let parent = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                .and_then(|ppid| ppid.parse().ok());
            let command = fs::read(entry.path().join("cmdline"))
                .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
                .unwrap_or_default();
            if let Some(parent) = parent {
                table.insert(pid, (parent, command));
            }
        }
        if !table.is_empty() {
            return table;
        }
    }
    let Ok(output) = Command::new("ps")
        .args(["-A", "-o", "pid=", "-o", "ppid=", "-o", "args="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return table;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.trim_start().splitn(2, char::is_whitespace);
        let pid = fields.next().and_then(|pid| pid.parse().ok());
        let mut rest = fields
            .next()
            .unwrap_or_default()
            .trim_start()
            .splitn(2, char::is_whitespace);
        let parent = rest.next().and_then(|ppid| ppid.parse().ok());
        if let (Some(pid), Some(parent)) = (pid, parent) {
            table.insert(
                pid,
                (parent, rest.next().unwrap_or_default().trim().to_string()),
            );
        }
    }
    table
}

/// Which of [`usage::CLIENTS`] a command line is, if any.
fn classify(command: &str) -> Option<&'static str> {
    let lower = command.to_ascii_lowercase();
    let mut words = lower.split_whitespace();
    let base = |word: &str| word.rsplit('/').next().unwrap_or(word).to_string();
    let program = base(words.next()?);
    let script = words.next().map(base).unwrap_or_default();
    if lower.contains("/chatgpt.app/") || program == "chatgpt" {
        return Some("chatgpt");
    }
    if lower.contains("/claude.app/") {
        return Some("claude-desktop");
    }
    if lower.contains("/cursor.app/") || program == "cursor" || program == "cursor-agent" {
        return Some("cursor");
    }
    if lower.contains("/codex.app/") || program == "codex" || program.starts_with("codex-") {
        return Some("codex");
    }
    if program == "claude" {
        return Some("claude-code");
    }
    if matches!(program.as_str(), "node" | "bun" | "deno") {
        if script == "claude" || lower.contains("/claude-code/") {
            return Some("claude-code");
        }
        if script == "codex" || script == "codex.js" || lower.contains("/@openai/codex/") {
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
            ("node server.js", None),
            ("", None),
        ];
        for (command, client) in cases {
            assert_eq!(classify(command), client, "{command}");
        }
    }

    #[test]
    fn the_process_table_knows_this_process_s_parent() {
        let table = processes();
        assert_eq!(
            table.get(&std::process::id()).map(|(parent, _)| *parent),
            Some(caller_pid())
        );
    }
}
