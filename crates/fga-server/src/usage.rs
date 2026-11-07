//! Anonymous daily usage counts (docs/telemetry.md): the report the gh wrapper and fga-server each
//! send at most once a day, the fixed vocabulary its `unsupported` counts come from, and
//! fga-server's own reporter. `contract/telemetry.json` holds every allowed value, and the collector
//! rejects anything else.

use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The report's shape. A change to it is a new schema, which the collector must accept first.
pub const SCHEMA: u32 = 1;
/// Where reports go: release builds only, unless an override names another address.
pub const ENDPOINT: &str = "https://telemetry.dirtydishes.dev/v1/report";
/// The release's version (the workspace's), which fga-server's own crate version isn't.
pub const PRODUCT_VERSION: &str = env!("FGA_PRODUCT_VERSION");
/// The longest a report's request may take.
pub const TIMEOUT: Duration = Duration::from_secs(2);
/// The largest count a report carries for one name; the collector refuses more.
pub const MAX_COUNT: u64 = 1_000_000;

/// One day's report. Nothing else is ever sent: no host, repository, path, argument, address or
/// identifier of any kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub schema: u32,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub mode: &'static str,
    /// The agent or app that ran gh, from [`CLIENTS`]; never in server mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge_major: Option<u64>,
    /// gh's `major.minor`; never in server mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gh: Option<String>,
    /// The ISO week of the first report, kept locally, so retention can be counted without an ID.
    pub first_week: String,
    /// Names from the vocabulary, each with how often the gateway lacked it since the last report.
    pub unsupported: BTreeMap<String, u64>,
}

pub const CLIENTS: [&str; 6] = [
    "codex",
    "chatgpt",
    "claude-code",
    "claude-desktop",
    "cursor",
    "other",
];

/// The version a report names: `X.Y.Z`, with a pre-release suffix only of the form `-alpha.N`,
/// `-beta.N` or `-rc.N`. Any other suffix or build text is dropped, so a custom build's version
/// string never leaves the machine.
pub fn report_version(version: &str) -> String {
    let without_build = version.split('+').next().unwrap_or_default();
    let (core, suffix) = without_build.split_once('-').unwrap_or((without_build, ""));
    let numbers: Vec<&str> = core.split('.').collect();
    let numeric = |part: &str| {
        !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit())
    };
    if numbers.len() != 3 || !numbers.iter().all(|part| numeric(part)) {
        return "0.0.0".to_string();
    }
    let pre_release = suffix
        .split_once('.')
        .is_some_and(|(stage, number)| matches!(stage, "alpha" | "beta" | "rc") && numeric(number));
    if pre_release {
        format!("{core}-{suffix}")
    } else {
        core.to_string()
    }
}

pub fn os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        _ => "other",
    }
}

pub fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => "other",
    }
}

#[derive(serde::Deserialize)]
struct Contract {
    graphql_types: Vec<String>,
    unsupported: HashSet<String>,
}

fn contract() -> &'static Contract {
    static CONTRACT: OnceLock<Contract> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        serde_json::from_str(include_str!("../../../contract/telemetry.json"))
            .expect("contract/telemetry.json parses")
    })
}

/// Why reports are off, if they are: `name=value` of the variable that turned them off. `own` is
/// the program's own switch, `GFJ_TELEMETRY` or `FGA_TELEMETRY`; `DO_NOT_TRACK` and `CI` count for
/// both. `CI` turns them off whenever it's set, even empty.
pub fn off_by_env(get: impl Fn(&str) -> Option<String>, own: &str) -> Option<String> {
    let set = |name: &str| {
        get(name)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
    };
    if let Some(value) =
        set(own).filter(|value| matches!(value.as_str(), "0" | "false" | "off" | "no"))
    {
        return Some(format!("{own}={value}"));
    }
    if let Some(value) =
        set("DO_NOT_TRACK").filter(|value| !matches!(value.as_str(), "0" | "false"))
    {
        return Some(format!("DO_NOT_TRACK={value}"));
    }
    if get("CI").is_some() {
        return Some("CI is set".to_string());
    }
    None
}

/// Where this build sends reports: `override_url` if set, else [`ENDPOINT`] in release builds. A
/// debug build sends nothing on its own, so tests and development runs never reach the collector.
pub fn endpoint(override_url: Option<String>) -> Option<String> {
    match override_url.filter(|url| !url.trim().is_empty()) {
        Some(url) => Some(url),
        None if cfg!(debug_assertions) => None,
        None => Some(ENDPOINT.to_string()),
    }
}

static UNSUPPORTED: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());
/// Whether the gateway counts what it lacks: only once reports are on.
static COUNTING: AtomicBool = AtomicBool::new(false);

/// Starts counting what the gateway lacks, for a process whose reports are on. Until then, nothing
/// is counted.
pub fn count_unsupported() {
    COUNTING.store(true, Ordering::Relaxed);
}

/// What the gateway lacked since the last call, emptying the count.
pub fn take_unsupported() -> BTreeMap<String, u64> {
    std::mem::take(
        &mut *UNSUPPORTED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

fn count(name: String) {
    if !COUNTING.load(Ordering::Relaxed) {
        return;
    }
    let mut counts = UNSUPPORTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let count = counts.entry(name).or_default();
    *count = (*count + 1).min(MAX_COUNT);
}

/// Counts a REST path the gateway has no route or method for.
pub(crate) fn rest_unsupported(path: &str) {
    count(rest_name(path));
}

/// Counts each field a GraphQL error says the gateway's schema lacks.
pub(crate) fn graphql_unsupported(message: &str) {
    if let Some(name) = graphql_name(message) {
        count(name);
    }
}

/// GitHub's name for a REST path, from the vocabulary: `rest:repos.<resource>` below a repository,
/// `rest:repos` for the repository itself, `rest:<resource>` at the top, else `rest:other`. The owner, repository and anything after the
/// resource are never part of it.
pub fn rest_name(path: &str) -> String {
    let mut segments = path.strip_prefix("/api/v3/").unwrap_or_default().split('/');
    let name = match segments.next() {
        Some("repos") => match segments.nth(2) {
            Some(resource) if !resource.is_empty() => format!("rest:repos.{resource}"),
            _ => "rest:repos".to_string(),
        },
        Some(resource) => format!("rest:{resource}"),
        None => String::new(),
    };
    known(name).unwrap_or_else(|| "rest:other".to_string())
}

/// GitHub's name for the field async-graphql's `Unknown field "x" on type "Y".` names:
/// `graphql:Y.x` if it's in the vocabulary, `graphql:Y.other` for one of the vocabulary's types,
/// else `graphql:other`. Other errors name nothing.
pub fn graphql_name(message: &str) -> Option<String> {
    let rest = message.strip_prefix("Unknown field \"")?;
    let (field, rest) = rest.split_once('"')?;
    let type_name = rest
        .strip_prefix(" on type \"")
        .and_then(|rest| rest.split_once('"'))
        .map(|(type_name, _)| type_name)
        .unwrap_or_default();
    Some(
        known(format!("graphql:{type_name}.{field}"))
            .or_else(|| {
                contract()
                    .graphql_types
                    .iter()
                    .any(|known| known == type_name)
                    .then(|| format!("graphql:{type_name}.other"))
            })
            .unwrap_or_else(|| "graphql:other".to_string()),
    )
}

fn known(name: String) -> Option<String> {
    contract().unsupported.contains(&name).then_some(name)
}

/// `name`, if it's in the vocabulary.
pub fn known_name(name: &str) -> Option<String> {
    known(name.to_string())
}

/// The forge's software and major version, from its `/api/forgejo/v1/version` answer (Forgejo
/// only) and its `/api/v1/version` answer (Forgejo and Gitea). Forgejo's own version reads
/// `15.0.11+gitea-1.22.0`; Gitea's `1.22.3`.
pub fn forge(
    forgejo_route: Option<&str>,
    api: Option<&str>,
) -> (Option<&'static str>, Option<u64>) {
    let major = |version: &str| {
        version
            .trim()
            .trim_start_matches('v')
            .split(['.', '+', '-'])
            .next()
            .and_then(|major| major.parse::<u64>().ok())
            .filter(|major| *major < 1000)
    };
    if let Some(version) = forgejo_route {
        return (Some("forgejo"), major(version));
    }
    match api {
        Some(version) if version.contains("+gitea-") => (Some("forgejo"), major(version)),
        Some(version) => match major(version) {
            Some(number) => (Some("gitea"), Some(number)),
            None => (Some("other"), None),
        },
        None => (None, None),
    }
}

/// Seconds since the Unix epoch, now.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The civil date of a day count since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The UTC date of `unix`, as `YYYY-MM-DD`.
pub fn utc_day(unix: u64) -> String {
    let (year, month, day) = civil((unix / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The ISO 8601 week of `unix`'s UTC date, as `YYYY-Www`.
pub fn iso_week(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    // 1970-01-01 was a Thursday; Monday is 0.
    let weekday = (days + 3).rem_euclid(7);
    let thursday = days - weekday + 3;
    let (year, _, _) = civil(thursday);
    let week = (thursday - days_from_civil(year, 1, 1)) / 7 + 1;
    format!("{year:04}-W{week:02}")
}

/// Whether `week` is a week as [`iso_week`] writes it.
pub fn is_week(week: &str) -> bool {
    week.len() == 8
        && week.as_bytes()[4..6] == *b"-W"
        && week[..4].bytes().all(|b| b.is_ascii_digit())
        && week[6..]
            .parse::<u32>()
            .is_ok_and(|w| (1..=53).contains(&w))
}

/// fga-server's reporter: once a day while it runs, from ten minutes after it starts, so a server
/// restarting in a loop sends nothing. The first week and the last day sent are kept in its state
/// directory ([`server_state_dir`]), so a restart neither sends twice in a day nor starts a new
/// first week; with none writable, they last as long as the process.
pub fn spawn_server_reporter(forgejo_url: url::Url) {
    let get = |name: &str| std::env::var(name).ok();
    if off_by_env(get, "FGA_TELEMETRY").is_some() {
        return;
    }
    let Some(endpoint) = endpoint(get("FGA_TELEMETRY_URL")) else {
        return;
    };
    let state = server_state_dir(get);
    count_unsupported();
    log(
        "fga-server sends one anonymous usage report a day (version, OS, forge; no hosts, \
         repositories or IDs). FGA_TELEMETRY=0 turns it off. See docs/telemetry.md.",
    );
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
        {
            Ok(client) => client,
            Err(_) => return,
        };
        tokio::time::sleep(Duration::from_secs(600)).await;
        let mut sent: Option<String> = None;
        loop {
            let today = utc_day(now());
            let already = sent.as_deref() == Some(today.as_str())
                || state
                    .as_ref()
                    .and_then(|dir| std::fs::read_to_string(dir.join("telemetry-sent")).ok())
                    .is_some_and(|day| day.trim() == today);
            if !already {
                let report = server_report(&client, &forgejo_url, state.as_deref()).await;
                // Sent or not, it's this day's one try; a failure stays silent.
                let _ = client.post(&endpoint).json(&report).send().await;
                if let Some(dir) = &state {
                    let _ = std::fs::write(dir.join("telemetry-sent"), &today);
                }
                sent = Some(today);
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

async fn server_report(
    client: &reqwest::Client,
    forgejo_url: &url::Url,
    state: Option<&std::path::Path>,
) -> Report {
    #[derive(serde::Deserialize)]
    struct Version {
        version: String,
    }
    let version = |path: &'static str| {
        let url = forgejo_url.join(path);
        async move {
            let response = client.get(url.ok()?).send().await.ok()?;
            if !response.status().is_success() {
                return None;
            }
            Some(response.json::<Version>().await.ok()?.version)
        }
    };
    let forgejo_route = version("api/forgejo/v1/version").await;
    let api = match forgejo_route {
        Some(_) => None,
        None => version("api/v1/version").await,
    };
    let (forge, forge_major) = forge(forgejo_route.as_deref(), api.as_deref());
    Report {
        schema: SCHEMA,
        version: report_version(PRODUCT_VERSION),
        os: os(),
        arch: arch(),
        mode: "server",
        client: None,
        forge,
        forge_major,
        gh: None,
        first_week: first_week(state),
        unsupported: take_unsupported(),
    }
}

/// Where fga-server keeps its report state: `FGA_STATE_DIR` (the image sets
/// `/var/lib/fga-server`), else `$XDG_STATE_HOME/fga-server` or `~/.local/state/fga-server`,
/// whichever can be created. `None` when none can.
fn server_state_dir(get: impl Fn(&str) -> Option<String>) -> Option<std::path::PathBuf> {
    let absolute = |value: Option<String>| {
        value
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let dir = absolute(get("FGA_STATE_DIR"))
        .or_else(|| absolute(get("XDG_STATE_HOME")).map(|base| base.join("fga-server")))
        .or_else(|| absolute(get("HOME")).map(|home| home.join(".local/state/fga-server")))?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The week of the first report, read from `state` or written there now.
fn first_week(state: Option<&std::path::Path>) -> String {
    static STARTED: OnceLock<String> = OnceLock::new();
    let this_week = STARTED.get_or_init(|| iso_week(now())).clone();
    let Some(dir) = state else {
        return this_week;
    };
    let path = dir.join("telemetry-first-week");
    match std::fs::read_to_string(&path) {
        Ok(week) if is_week(week.trim()) => week.trim().to_string(),
        _ => {
            let _ = std::fs::write(&path, &this_week);
            this_week
        }
    }
}

/// One line in the server's log. A log that can't be written loses the line, never the server.
fn log(message: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weeks_and_days_follow_iso_8601() {
        let at = |year, month, day| days_from_civil(year, month, day) as u64 * 86_400;
        assert_eq!(utc_day(at(2026, 11, 7)), "2026-11-07");
        assert_eq!(iso_week(at(2026, 11, 7)), "2026-W45");
        // 2027-01-01 is a Friday, so it's in 2026's last week.
        assert_eq!(iso_week(at(2027, 1, 1)), "2026-W53");
        // 2024-12-30 is a Monday in 2025's first week.
        assert_eq!(iso_week(at(2024, 12, 30)), "2025-W01");
        assert_eq!(iso_week(at(2021, 1, 3)), "2020-W53");
        assert_eq!(iso_week(0), "1970-W01");
        assert!(is_week("2026-W45") && !is_week("2026-W54") && !is_week("26-W45"));
    }

    #[test]
    fn unsupported_names_come_from_the_vocabulary_only() {
        assert_eq!(
            rest_name("/api/v3/repos/acme/secret-repo/releases/12"),
            "rest:repos.releases"
        );
        assert_eq!(
            rest_name("/api/v3/repos/acme/secret-repo/acme-private-thing"),
            "rest:other"
        );
        assert_eq!(rest_name("/api/v3/repos/acme/secret-repo"), "rest:repos");
        assert_eq!(rest_name("/api/v3/gists/abc"), "rest:gists");
        assert_eq!(rest_name("/api/v3/acme"), "rest:other");
        assert_eq!(rest_name("/elsewhere"), "rest:other");
        assert_eq!(
            graphql_name(
                r#"Unknown field "projectsV2" on type "Repository". Did you mean "projects"?"#
            ),
            Some("graphql:Repository.projectsV2".to_string())
        );
        assert_eq!(
            graphql_name(r#"Unknown field "acmeSecret" on type "Repository"."#),
            Some("graphql:Repository.other".to_string())
        );
        assert_eq!(
            graphql_name(r#"Unknown field "x" on type "AcmeType"."#),
            Some("graphql:other".to_string())
        );
        assert_eq!(graphql_name("Unknown argument \"x\""), None);
    }

    #[test]
    fn a_version_keeps_only_numbers_and_a_plain_pre_release() {
        assert_eq!(report_version("0.3.0"), "0.3.0");
        assert_eq!(report_version("0.3.0-rc.1"), "0.3.0-rc.1");
        assert_eq!(report_version("0.3.0-alice.private.forge"), "0.3.0");
        assert_eq!(report_version("0.3.0+build.acme"), "0.3.0");
        assert_eq!(report_version("0.3.0-rc.1+acme"), "0.3.0-rc.1");
        assert_eq!(report_version("acme-build"), "0.0.0");
        assert_eq!(report_version("1.2"), "0.0.0");
    }

    #[test]
    fn the_forge_comes_from_its_version_answers() {
        assert_eq!(
            forge(Some("15.0.11+gitea-1.22.0"), None),
            (Some("forgejo"), Some(15))
        );
        assert_eq!(
            forge(None, Some("11.0.2+gitea-1.22.0")),
            (Some("forgejo"), Some(11))
        );
        assert_eq!(forge(None, Some("1.22.3")), (Some("gitea"), Some(1)));
        assert_eq!(forge(None, Some("unknown")), (Some("other"), None));
        assert_eq!(forge(None, None), (None, None));
    }

    #[test]
    fn reports_stop_for_each_off_switch() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(off_by_env(env(&[]), "GFJ_TELEMETRY"), None);
        assert_eq!(
            off_by_env(env(&[("GFJ_TELEMETRY", "0")]), "GFJ_TELEMETRY"),
            Some("GFJ_TELEMETRY=0".to_string())
        );
        assert_eq!(
            off_by_env(env(&[("GFJ_TELEMETRY", "1")]), "GFJ_TELEMETRY"),
            None
        );
        assert_eq!(
            off_by_env(env(&[("DO_NOT_TRACK", "1")]), "FGA_TELEMETRY"),
            Some("DO_NOT_TRACK=1".to_string())
        );
        assert_eq!(
            off_by_env(env(&[("DO_NOT_TRACK", "0")]), "FGA_TELEMETRY"),
            None
        );
        assert_eq!(
            off_by_env(env(&[("CI", "true")]), "GFJ_TELEMETRY"),
            Some("CI is set".to_string())
        );
        // Set at all, even empty or "false", it turns reports off.
        for set in [
            off_by_env(env(&[("CI", "")]), "GFJ_TELEMETRY"),
            off_by_env(env(&[("CI", "0")]), "GFJ_TELEMETRY"),
            off_by_env(env(&[("CI", "false")]), "GFJ_TELEMETRY"),
        ] {
            assert_eq!(set, Some("CI is set".to_string()));
        }
    }

    #[test]
    fn the_contract_lists_this_code_s_values() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../contract/telemetry.json")).unwrap();
        assert_eq!(contract["schema"], SCHEMA);
        assert_eq!(contract["client"], serde_json::json!(CLIENTS));
        for (field, value) in [("os", os()), ("arch", arch())] {
            assert!(
                contract[field]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v == value),
                "{field} {value}"
            );
        }
    }
}
