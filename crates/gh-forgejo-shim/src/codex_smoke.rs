//! Codex probe smoke checks.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

pub const CODEX_PR_BOARD_FIELDS: &str = "additions,baseRefName,createdAt,deletions,headRefName,isDraft,number,state,title,updatedAt,url,mergeStateStatus,mergeable,statusCheckRollup";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeProbe {
    pub name: &'static str,
    pub command: Vec<&'static str>,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SmokeResult {
    pub probe: SmokeProbe,
    pub exit_code: i32,
    pub duration_ms: f64,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stderr_excerpt: String,
}

impl SmokeResult {
    pub fn ok(&self) -> bool {
        self.exit_code == 0 || self.probe.optional
    }
}

pub fn codex_smoke_probes() -> Vec<SmokeProbe> {
    vec![
        SmokeProbe {
            name: "gh version",
            command: vec!["gh", "--version"],
            optional: false,
        },
        SmokeProbe {
            name: "gh auth status",
            command: vec!["gh", "auth", "status"],
            optional: false,
        },
        SmokeProbe {
            name: "gh api user",
            command: vec!["gh", "api", "user"],
            optional: false,
        },
        SmokeProbe {
            name: "gh pr status",
            command: vec![
                "gh",
                "pr",
                "status",
                "--json",
                "number,title,url,headRefName,statusCheckRollup",
            ],
            optional: true,
        },
        SmokeProbe {
            name: "gh pr list",
            command: vec![
                "gh",
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                "10",
                "--json",
                CODEX_PR_BOARD_FIELDS,
            ],
            optional: true,
        },
        SmokeProbe {
            name: "git repo root",
            command: vec!["git", "rev-parse", "--show-toplevel"],
            optional: false,
        },
        SmokeProbe {
            name: "git remotes",
            command: vec!["git", "remote", "-v"],
            optional: false,
        },
        SmokeProbe {
            name: "git default branch",
            command: vec![
                "git",
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
            optional: true,
        },
        SmokeProbe {
            name: "git current branch",
            command: vec!["git", "branch", "--show-current"],
            optional: true,
        },
        SmokeProbe {
            name: "git upstream",
            command: vec![
                "git",
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{u}",
            ],
            optional: true,
        },
        SmokeProbe {
            name: "git local and remote refs",
            command: vec![
                "git",
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads",
                "refs/remotes",
            ],
            optional: true,
        },
    ]
}

pub fn run_codex_smoke(cwd: Option<&Path>, env: &HashMap<String, String>) -> Vec<SmokeResult> {
    codex_smoke_probes()
        .into_iter()
        .map(|probe| run_probe(probe, cwd, env))
        .collect()
}

pub fn format_codex_smoke(results: &[SmokeResult]) -> String {
    let mut lines = vec!["Codex probe smoke results:".to_string()];
    for result in results {
        let marker = if result.exit_code == 0 {
            "ok"
        } else if result.probe.optional {
            "optional-fail"
        } else {
            "fail"
        };
        lines.push(format!(
            "[{marker}] {}: exit {}, {:.0} ms, stdout {} B, stderr {} B",
            result.probe.name,
            result.exit_code,
            result.duration_ms,
            result.stdout_bytes,
            result.stderr_bytes
        ));
        if !result.stderr_excerpt.is_empty() {
            lines.push(format!("      stderr: {}", result.stderr_excerpt));
        }
    }

    let required_failures = results
        .iter()
        .filter(|result| result.exit_code != 0 && !result.probe.optional)
        .count();
    if required_failures == 0 {
        lines.push("Required probes passed.".to_string());
    } else {
        lines.push(format!("Required probe failures: {required_failures}"));
    }
    lines.join("\n")
}

pub fn required_failures(results: &[SmokeResult]) -> usize {
    results
        .iter()
        .filter(|result| result.exit_code != 0 && !result.probe.optional)
        .count()
}

fn run_probe(probe: SmokeProbe, cwd: Option<&Path>, env: &HashMap<String, String>) -> SmokeResult {
    let started = Instant::now();
    let mut command = Command::new(probe.command[0]);
    command.args(&probe.command[1..]).env_clear().envs(env);
    if let Some(path) = cwd {
        command.current_dir(path);
    }

    let (exit_code, stdout, stderr) = match command.output() {
        Ok(output) => (
            output.status.code().unwrap_or(1),
            output.stdout,
            output.stderr,
        ),
        Err(error) => (127, Vec::new(), error.to_string().into_bytes()),
    };

    SmokeResult {
        probe,
        exit_code,
        duration_ms: started.elapsed().as_secs_f64() * 1000.0,
        stdout_bytes: stdout.len(),
        stderr_bytes: stderr.len(),
        stderr_excerpt: excerpt(&String::from_utf8_lossy(&stderr)),
    }
}

fn excerpt(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() <= 240 {
        return compact;
    }
    format!("{}...", &compact[..237])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_sequence_includes_codex_gh_and_git_checks() {
        let commands = codex_smoke_probes()
            .into_iter()
            .map(|probe| probe.command.join(" "))
            .collect::<Vec<_>>();

        assert!(commands.contains(&"gh --version".to_string()));
        assert!(commands.contains(&"gh auth status".to_string()));
        assert!(commands.contains(&"git rev-parse --show-toplevel".to_string()));
        assert!(
            commands.contains(&"git rev-parse --abbrev-ref --symbolic-full-name @{u}".to_string())
        );
    }

    #[test]
    fn formatter_marks_required_and_optional_failures() {
        let results = vec![
            SmokeResult {
                probe: SmokeProbe {
                    name: "gh version",
                    command: vec!["gh", "--version"],
                    optional: false,
                },
                exit_code: 127,
                duration_ms: 1.0,
                stdout_bytes: 0,
                stderr_bytes: 10,
                stderr_excerpt: "missing gh".to_string(),
            },
            SmokeResult {
                probe: SmokeProbe {
                    name: "gh pr status",
                    command: vec!["gh", "pr", "status"],
                    optional: true,
                },
                exit_code: 1,
                duration_ms: 1.0,
                stdout_bytes: 0,
                stderr_bytes: 10,
                stderr_excerpt: "no pull request".to_string(),
            },
        ];

        let output = format_codex_smoke(&results);

        assert!(output.contains("[fail] gh version"));
        assert!(output.contains("[optional-fail] gh pr status"));
        assert!(output.contains("Required probe failures: 1"));
        assert_eq!(required_failures(&results), 1);
    }
}
