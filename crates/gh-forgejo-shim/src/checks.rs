//! gh 2.103.3's non-TTY checks presentation over the gateway's shared projections.
//!
//! Upstream sources at https://github.com/cli/cli/tree/v2.103.3/pkg/cmd/pr/checks:
//! - aggregate.go:36-119: rows, state/bucket mapping, required filter, deduplication.
//! - output.go:11-64,69-132: durations, non-TTY columns, TTY-only summaries, ordering.
//! - checks.go:189-190,248-252,302-308: JSON exits, table exits, no-checks errors.

use std::collections::HashSet;
use std::io::Write;

use fga_core::forgejo::{CommitStatus, Protection};
use fga_core::map::{self, Check, CheckRunState, CheckStatusState, StatusState};
use serde::Serialize;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::forgejo::RepoRef;
use crate::{Result, ShimError};

const ZERO_TIME: &str = "0001-01-01T00:00:00Z";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckRow {
    name: String,
    state: &'static str,
    started_at: String,
    completed_at: String,
    link: String,
    bucket: &'static str,
    event: String,
    workflow: String,
    description: String,
}

pub(crate) fn aggregate(
    statuses: &[CommitStatus],
    repo: &RepoRef,
    required: bool,
    protection: Option<&Protection>,
) -> Result<Vec<CheckRow>> {
    let public = format!("https://{}/", repo.host)
        .parse::<reqwest::Url>()
        .map_err(|error| ShimError::new(format!("invalid Forgejo public URL: {error}")))?;
    let mut mapped = map::checks(statuses, &repo.owner, &repo.repo, &public);

    // aggregate.go:95-119 dedupes newest CheckRuns by job/workflow/event, and
    // StatusContexts by context. StatusContexts have no StartedAt in gh's DTO.
    mapped.sort_by_key(|check| std::cmp::Reverse(started_at(check)));
    let mut actions = HashSet::new();
    let mut contexts = HashSet::new();
    let mut rows = Vec::new();
    for check in mapped {
        let unique = match &check {
            Check::Action {
                job,
                workflow,
                event,
                ..
            } => actions.insert(format!("{job}/{workflow}/{event}")),
            Check::Status { source, .. } => contexts.insert(source.context.clone()),
        };
        if !unique || (required && !map::required(&check.source().context, protection)) {
            continue;
        }
        rows.push(CheckRow::from_check(check)?);
    }
    Ok(rows)
}

fn started_at(check: &Check) -> Option<OffsetDateTime> {
    match check {
        Check::Action { source, .. } => OffsetDateTime::parse(&source.created_at, &Rfc3339).ok(),
        Check::Status { .. } => None,
    }
}

impl CheckRow {
    fn from_check(check: Check) -> Result<Self> {
        let state = match &check {
            Check::Action { .. } => match check.run_state() {
                Some(CheckRunState::ActionRequired) => "ACTION_REQUIRED",
                Some(CheckRunState::Cancelled) => "CANCELLED",
                Some(CheckRunState::Completed) => "", // COMPLETED without a conclusion
                Some(CheckRunState::Failure) => "FAILURE",
                Some(CheckRunState::InProgress) => "IN_PROGRESS",
                Some(CheckRunState::Neutral) => "NEUTRAL",
                Some(CheckRunState::Pending) => "PENDING",
                Some(CheckRunState::Queued) => "QUEUED",
                Some(CheckRunState::Skipped) => "SKIPPED",
                Some(CheckRunState::Stale) => "STALE",
                Some(CheckRunState::StartupFailure) => "STARTUP_FAILURE",
                Some(CheckRunState::Success) => "SUCCESS",
                Some(CheckRunState::TimedOut) => "TIMED_OUT",
                Some(CheckRunState::Waiting) => "WAITING",
                None => unreachable!("an Action always has a run state"),
            },
            Check::Status { state, .. } => match state {
                StatusState::Expected => "EXPECTED",
                StatusState::Error => "ERROR",
                StatusState::Failure => "FAILURE",
                StatusState::Pending => "PENDING",
                StatusState::Success => "SUCCESS",
            },
        };
        let bucket = bucket(state);
        Ok(match check {
            Check::Action {
                source,
                workflow,
                job,
                event,
                details_url,
                status,
                ..
            } => Self {
                name: job,
                state,
                started_at: timestamp(&source.created_at)?,
                completed_at: if status == CheckStatusState::Completed {
                    timestamp(&source.updated_at)?
                } else {
                    ZERO_TIME.into()
                },
                link: details_url.to_string(),
                bucket,
                event,
                workflow,
                // The gateway CheckRun has no description field. StatusContext does.
                description: String::new(),
            },
            Check::Status {
                source, target_url, ..
            } => Self {
                name: source.context,
                state,
                started_at: ZERO_TIME.into(),
                completed_at: ZERO_TIME.into(),
                link: target_url.map(|url| url.to_string()).unwrap_or_default(),
                bucket,
                event: String::new(),
                workflow: String::new(),
                description: source.description.unwrap_or_default(),
            },
        })
    }

    pub(crate) fn export(&self, fields: &[String]) -> Result<Value> {
        let mut value = serde_json::to_value(self)
            .map_err(|error| ShimError::new(format!("could not serialize check: {error}")))?;
        if let Value::Object(object) = &mut value {
            object.retain(|key, _| fields.contains(key));
        }
        Ok(value)
    }
}

fn timestamp(raw: &str) -> Result<String> {
    let parsed = OffsetDateTime::parse(raw, &Rfc3339)
        .map_err(|error| ShimError::new(format!("invalid check timestamp {raw:?}: {error}")))?;
    parsed
        .format(&Rfc3339)
        .map_err(|error| ShimError::new(format!("invalid check timestamp {raw:?}: {error}")))
}

// aggregate.go:72-88. In particular, STALE and STARTUP_FAILURE stay pending.
fn bucket(state: &str) -> &'static str {
    match state {
        "SUCCESS" => "pass",
        "SKIPPED" | "NEUTRAL" => "skipping",
        "ERROR" | "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" => "fail",
        "CANCELLED" => "cancel",
        _ => "pending",
    }
}

pub(crate) fn print(mut rows: Vec<CheckRow>, stdout: &mut dyn Write) -> Result<i32> {
    // Port of Go 1.27.3 src/sort/zsortfunc.go:9-16, insertionSort_func:
    // https://github.com/golang/go/blob/go1.27.3/src/sort/zsortfunc.go#L9-L16
    // gh's output.go:104-120 less predicate is not a total order, so Rust's
    // sort_by can panic. Keep that predicate, including the literal "success".
    // Go uses insertion sort for <=12 rows and pdqsort above that (:61-74).
    // We use insertion sort for all lengths; mixed-bucket order may differ from
    // gh above 12 rows.
    let less = |a: &CheckRow, b: &CheckRow| {
        if a.bucket == b.bucket {
            if a.name == b.name {
                return a.link < b.link;
            }
            return a.name < b.name;
        }
        a.bucket == "fail" || (a.bucket == "pending" && b.bucket == "success")
    };
    for i in 1..rows.len() {
        let mut j = i;
        while j > 0 && less(&rows[j], &rows[j - 1]) {
            rows.swap(j, j - 1);
            j -= 1;
        }
    }
    for row in &rows {
        // output.go:50-64. Non-TTY output has no headers or summary, and retains
        // the empty description column and its preceding tab.
        writeln!(
            stdout,
            "{}\t{}\t{}\t{}\t{}",
            row.name,
            if row.bucket == "cancel" {
                "fail"
            } else {
                row.bucket
            },
            elapsed(&row.started_at, &row.completed_at),
            row.link,
            row.description
        )?;
    }
    // checks.go:189-190 exports JSON before this check; 248-252 set table exits.
    Ok(if rows.iter().any(|row| row.bucket == "fail") {
        1
    } else if rows.iter().any(|row| row.bucket == "pending") {
        8
    } else {
        0
    })
}

fn elapsed(start: &str, end: &str) -> String {
    if start == ZERO_TIME || end == ZERO_TIME {
        return "0".into();
    }
    let (Ok(start), Ok(end)) = (
        OffsetDateTime::parse(start, &Rfc3339),
        OffsetDateTime::parse(end, &Rfc3339),
    ) else {
        return "0".into();
    };
    // Go's time.Duration saturates at i64 nanoseconds and prints hours, never days.
    let nanos = (end - start).whole_nanoseconds().min(i64::MAX as i128);
    if nanos <= 0 {
        return "0".into();
    }
    let unit = if nanos < 1_000 {
        (1, "ns")
    } else if nanos < 1_000_000 {
        (1_000, "µs")
    } else if nanos < 1_000_000_000 {
        (1_000_000, "ms")
    } else {
        (1_000_000_000, "s")
    };
    let hours = nanos / 3_600_000_000_000;
    let minutes = nanos / 60_000_000_000 % 60;
    let seconds = nanos / unit.0 % if unit.1 == "s" { 60 } else { 1_000 };
    let fraction = nanos % unit.0;
    let mut result = String::new();
    if hours > 0 {
        result.push_str(&format!("{hours}h"));
    }
    if hours > 0 || minutes > 0 {
        result.push_str(&format!("{minutes}m"));
    }
    result.push_str(&seconds.to_string());
    if fraction > 0 {
        let width = unit.0.ilog10() as usize;
        result.push('.');
        result.push_str(format!("{fraction:0width$}").trim_end_matches('0'));
    }
    result.push_str(unit.1);
    result
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;
    use serde_json::json;

    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

    fn status(id: i64, context: &str, state: &str, description: &str) -> CommitStatus {
        serde_json::from_value(json!({
            "id": id, "context": context, "status": state, "description": description,
            "target_url": "/owner/repo/actions/runs/1/jobs/1",
            "created_at": "2026-06-01T00:00:00Z", "updated_at": "2026-06-01T00:01:02.1234Z"
        }))
        .expect("status fixture")
    }

    #[test]
    fn shared_mapping_supplies_actions_and_latest_contexts() -> TestResult {
        let repo = RepoRef::new("forge.test", "owner", "repo");
        let statuses = [
            status(1, "ci / test (pull_request)", "failure", "old result"),
            status(
                2,
                "ci / test (pull_request)",
                "success",
                "Has finished running",
            ),
            status(
                3,
                "ci / skipped (pull_request)",
                "success",
                "Has been skipped",
            ),
            status(4, "ci / neutral (pull_request)", "warning", "warning"),
            status(5, "ci / queued (pull_request)", "pending", "Waiting to run"),
            status(
                6,
                "ci / running (pull_request)",
                "pending",
                "Has started running",
            ),
        ];
        let rows = aggregate(&statuses, &repo, false, None)?;
        assert_eq!(rows.len(), 5, "fga-core must discard the old failure");
        let test = rows
            .iter()
            .find(|row| row.name == "test")
            .expect("test row");
        assert_eq!(
            test.export(
                &[
                    "name",
                    "state",
                    "bucket",
                    "startedAt",
                    "completedAt",
                    "workflow",
                    "event",
                    "description",
                    "link"
                ]
                .map(String::from)
            )?,
            json!({
                "name":"test", "state":"SUCCESS", "bucket":"pass", "workflow":"ci", "event":"pull_request",
                "startedAt":"2026-06-01T00:00:00Z", "completedAt":"2026-06-01T00:01:02.1234Z",
                "description":"", "link":"https://forge.test/owner/repo/actions/runs/1/jobs/1"
            })
        );
        for (name, state, bucket) in [
            ("skipped", "SKIPPED", "skipping"),
            ("neutral", "NEUTRAL", "skipping"),
            ("queued", "QUEUED", "pending"),
            ("running", "IN_PROGRESS", "pending"),
        ] {
            let row = rows.iter().find(|row| row.name == name).expect("row");
            assert_eq!((row.state, row.bucket), (state, bucket));
            if bucket == "pending" {
                assert_eq!(row.completed_at, ZERO_TIME);
            }
        }
        Ok(())
    }

    #[test]
    fn status_contexts_have_zero_times_and_preserve_descriptions() -> TestResult {
        let repo = RepoRef::new("forge.test", "owner", "repo");
        let statuses = [CommitStatus {
            target_url: "checks/1".into(),
            ..status(1, "external", "warning", "external warning")
        }];
        let rows = aggregate(&statuses, &repo, false, None)?;
        assert_eq!(
            rows[0].export(
                &[
                    "name",
                    "state",
                    "bucket",
                    "startedAt",
                    "completedAt",
                    "workflow",
                    "event",
                    "description",
                    "link"
                ]
                .map(String::from)
            )?,
            json!({
                "name":"external", "state":"SUCCESS", "bucket":"pass", "workflow":"", "event":"",
                "startedAt":ZERO_TIME, "completedAt":ZERO_TIME,
                "description":"external warning", "link":"https://forge.test/checks/1"
            })
        );
        let mut output = Vec::new();
        assert_eq!(print(rows, &mut output)?, 0);
        assert_eq!(
            String::from_utf8(output)?,
            "external\tpass\t0\thttps://forge.test/checks/1\texternal warning\n"
        );
        Ok(())
    }

    #[test]
    fn table_orders_failures_then_names_and_links_without_a_summary() -> TestResult {
        let repo = RepoRef::new("forge.test", "owner", "repo");
        let statuses = [
            status(1, "ci / z (pull_request)", "success", "done"),
            status(2, "ci / b (pull_request)", "failure", "done"),
            status(3, "ci / a (pull_request)", "failure", "done"),
        ];
        let mut output = Vec::new();
        assert_eq!(
            print(aggregate(&statuses, &repo, false, None)?, &mut output)?,
            1
        );
        assert_eq!(
            String::from_utf8(output)?,
            concat!(
                "a\tfail\t1m2.1234s\thttps://forge.test/owner/repo/actions/runs/1/jobs/1\t\n",
                "b\tfail\t1m2.1234s\thttps://forge.test/owner/repo/actions/runs/1/jobs/1\t\n",
                "z\tpass\t1m2.1234s\thttps://forge.test/owner/repo/actions/runs/1/jobs/1\t\n"
            )
        );
        Ok(())
    }

    fn row(name: String, bucket: &'static str, link: String) -> CheckRow {
        CheckRow {
            name,
            bucket,
            link,
            state: "",
            started_at: ZERO_TIME.into(),
            completed_at: ZERO_TIME.into(),
            event: String::new(),
            workflow: String::new(),
            description: String::new(),
        }
    }

    #[test]
    fn table_sort_does_not_panic_on_seeded_mixed_buckets() -> TestResult {
        let mut seed = 0x8393_c20f_a21d_4148_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as usize
        };
        let buckets = ["pass", "pending", "skipping", "cancel", "fail"];
        let mut mixed_sets = 0;
        for _ in 0..4_096 {
            let len = next() % 65;
            let rows: Vec<_> = (0..len)
                .map(|_| {
                    let bucket = buckets[next() % buckets.len()];
                    row(
                        format!("check-{}", next() % 16),
                        bucket,
                        format!("https://forge.test/{}", next() % 8),
                    )
                })
                .collect();
            if rows
                .iter()
                .map(|row| row.bucket)
                .collect::<HashSet<_>>()
                .len()
                > 1
            {
                mixed_sets += 1;
            }
            print(rows, &mut std::io::sink())?;
        }
        assert!(mixed_sets >= 1_000);
        Ok(())
    }

    #[test]
    fn table_pins_gh_insertion_order_for_mixed_buckets() -> TestResult {
        // Hand-step Go's insertion sort on [z/pass, b/pending, a/pass,
        // s/skipping, f/fail]:
        // i=1: b/pending is not less than z/pass, so stop.
        // i=2: a/pass is not less than b/pending, so stop before reaching z.
        // i=3: s/skipping is not less than a/pass, so stop.
        // i=4: f/fail swaps past s, a, b, then z to reach the front.
        // Final order: [f/fail, z/pass, b/pending, a/pass, s/skipping].
        let rows = [
            ("z", "pass"),
            ("b", "pending"),
            ("a", "pass"),
            ("s", "skipping"),
            ("f", "fail"),
        ]
        .map(|(name, bucket)| row(name.into(), bucket, String::new()))
        .into();
        let mut output = Vec::new();
        assert_eq!(print(rows, &mut output)?, 1);
        assert_eq!(
            String::from_utf8(output)?,
            concat!(
                "f\tfail\t0\t\t\n",
                "z\tpass\t0\t\t\n",
                "b\tpending\t0\t\t\n",
                "a\tpass\t0\t\t\n",
                "s\tskipping\t0\t\t\n"
            )
        );
        Ok(())
    }

    #[test]
    fn aggregate_started_at_sort_is_total_and_newest_first() -> TestResult {
        let repo = RepoRef::new("forge.test", "owner", "repo");
        let statuses: Vec<_> = [
            ("oldest", "2026-06-01T00:00:00Z"),
            ("newest", "2026-06-03T00:00:00Z"),
            ("tied-a", "2026-06-02T00:00:00Z"),
            ("tied-b", "2026-06-02T01:00:00+01:00"),
            ("external-a", ""),
            ("external-b", ""),
        ]
        .into_iter()
        .enumerate()
        .map(|(id, (name, created_at))| {
            let context = if created_at.is_empty() {
                name.into()
            } else {
                format!("ci / {name} (pull_request)")
            };
            CommitStatus {
                created_at: created_at.into(),
                ..status(id as i64, &context, "success", "done")
            }
        })
        .collect();
        let public = "https://forge.test/".parse()?;
        let mapped = map::checks(&statuses, &repo.owner, &repo.repo, &public);
        // The actual dedupe keys cover distinct instants, equal instants with
        // different UTC offsets, and None for StatusContexts. Reverse preserves
        // Option<OffsetDateTime>'s total order, including equality and None.
        let keys: Vec<_> = mapped
            .iter()
            .map(|check| std::cmp::Reverse(started_at(check)))
            .collect();
        for a in &keys {
            assert_eq!(a.cmp(a), Ordering::Equal);
            for b in &keys {
                assert_eq!(a.cmp(b), b.cmp(a).reverse());
                for c in &keys {
                    if a <= b && b <= c {
                        assert!(a <= c);
                    }
                }
            }
        }
        let rows = aggregate(&statuses, &repo, false, None)?;
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            [
                "newest",
                "tied-a",
                "tied-b",
                "oldest",
                "external-a",
                "external-b"
            ]
        );
        Ok(())
    }

    #[test]
    fn gh_buckets_include_cancel_and_unfinished_conclusions() {
        for (state, expected) in [
            ("SUCCESS", "pass"),
            ("SKIPPED", "skipping"),
            ("NEUTRAL", "skipping"),
            ("ERROR", "fail"),
            ("FAILURE", "fail"),
            ("TIMED_OUT", "fail"),
            ("ACTION_REQUIRED", "fail"),
            ("CANCELLED", "cancel"),
            ("STALE", "pending"),
            ("STARTUP_FAILURE", "pending"),
            ("QUEUED", "pending"),
            ("IN_PROGRESS", "pending"),
            ("EXPECTED", "pending"),
        ] {
            assert_eq!(bucket(state), expected, "{state}");
        }
    }

    #[test]
    fn durations_follow_go_units_precision_and_saturation() {
        for (start, end, expected) in [
            (ZERO_TIME, "2026-06-01T00:00:01Z", "0"),
            ("2026-06-01T00:00:00Z", ZERO_TIME, "0"),
            ("2026-06-01T00:00:00Z", "2026-06-01T00:00:00Z", "0"),
            ("2026-06-01T00:00:01Z", "2026-06-01T00:00:00Z", "0"),
            (
                "2026-06-01T00:00:00Z",
                "2026-06-01T00:00:00.000000001Z",
                "1ns",
            ),
            (
                "2026-06-01T00:00:00Z",
                "2026-06-01T00:00:00.0000012Z",
                "1.2µs",
            ),
            (
                "2026-06-01T00:00:00Z",
                "2026-06-01T00:00:00.001002Z",
                "1.002ms",
            ),
            (
                "2026-06-01T00:00:00Z",
                "2026-06-01T00:00:01.000000001Z",
                "1.000000001s",
            ),
            ("2026-06-01T00:00:00Z", "2026-06-01T01:00:00Z", "1h0m0s"),
            ("2026-06-01T00:00:00Z", "2026-06-02T00:01:02Z", "24h1m2s"),
            ("2026-06-01T01:00:00+01:00", "2026-06-01T00:00:01Z", "1s"),
            (
                "0001-01-01T00:00:01Z",
                "9999-01-01T00:00:00Z",
                "2562047h47m16.854775807s",
            ),
        ] {
            assert_eq!(elapsed(start, end), expected, "{start} -> {end}");
        }
    }
}
