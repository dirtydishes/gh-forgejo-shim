use super::*;
use crate::forgejo::{CommitStatus, CommitStatusState as S, Protection, PullRequest, Review};
use serde_json::json;

/// `bodyHTML` as a forge at `https://forge.example` serves it.
fn body_html(body: &str) -> String {
    super::body_html(body, Some(&"https://forge.example".parse().unwrap()))
}

fn pr() -> PullRequest {
    serde_json::from_value(json!({"number":1,"title":"scenario","state":"open","merged":false,"draft":false,"mergeable":true,"html_url":"https://forge.test/o/r/pulls/1","head":{"label":"s/pass","ref":"s/pass","sha":"abc","repo":null},"base":{"label":"main","ref":"main","sha":"def","repo":null},"user":{"id":1,"login":"o","avatar_url":""},"merged_at":null})).unwrap()
}
fn status(context: &str, state: S, description: &str) -> CommitStatus {
    CommitStatus {
        id: 1,
        context: context.into(),
        state,
        description: Some(description.into()),
        target_url: "/o/r/actions/runs/5/jobs/1".into(),
        created_at: "2026-10-30T19:00:00Z".into(),
        updated_at: "2026-10-30T19:00:02Z".into(),
    }
}
fn mapped(rows: &[CommitStatus]) -> Vec<Check> {
    checks(rows, "o", "r", &"https://forge.test".parse().unwrap())
}
fn review(id: i64, user: i64, state: &str) -> Review {
    serde_json::from_value(json!({"id":id,"user":{"id":user,"login":format!("u{user}"),"avatar_url":""},"body":"","state":state,"submitted_at":format!("2026-10-30T19:00:{id:02}Z"),"commit_id":"abc"})).unwrap()
}

#[test]
fn pull_state_draft_and_mergeability_table() {
    for (closed, merged, mergeable_flag, expected_state, expected_mergeable) in [
        (
            false,
            false,
            true,
            PullRequestState::Open,
            MergeableState::Mergeable,
        ),
        (
            false,
            false,
            false,
            PullRequestState::Open,
            MergeableState::Unknown,
        ),
        (
            true,
            false,
            true,
            PullRequestState::Closed,
            MergeableState::Unknown,
        ),
        (
            true,
            true,
            true,
            PullRequestState::Merged,
            MergeableState::Unknown,
        ),
        (
            false,
            true,
            false,
            PullRequestState::Merged,
            MergeableState::Unknown,
        ),
    ] {
        let mut p = pr();
        p.state = if closed { "closed" } else { "open" }.into();
        p.merged = merged;
        p.mergeable = mergeable_flag;
        assert_eq!(state(&p), expected_state);
        for draft in [false, true] {
            p.draft = draft;
            assert_eq!(pull_request(&p, &WipPrefixes::default())["draft"], draft);
            assert_eq!(
                mergeable(&p),
                if draft {
                    MergeableState::Unknown
                } else {
                    expected_mergeable
                }
            );
        }
    }
}

#[test]
fn draft_title_prefix_table() {
    for (draft, source, configured, expected) in [
        (true, "WIP: scenario", None, "scenario"),
        (true, "wIp:\t  scenario", None, "scenario"),
        (true, "[wip]\u{2003}scenario", None, "scenario"),
        (true, "[WIP]scenario", None, "scenario"),
        (true, "WIP:", None, ""),
        (true, "scenario WIP: title", None, "scenario WIP: title"),
        (true, " WIP: scenario", None, " WIP: scenario"),
        (true, "WIP: [WIP] scenario", None, "[WIP] scenario"),
        (true, "[WIP]é", None, "é"),
        (true, "éé", None, "éé"),
        (false, "WIP: scenario", None, "WIP: scenario"),
        (
            true,
            "draft: scenario",
            Some(" draft: , [DRAFT] , "),
            "scenario",
        ),
        (true, "[draft] scenario", Some("draft:,[DRAFT]"), "scenario"),
        (true, "WIP: scenario", Some("draft:"), "WIP: scenario"),
        (true, "WIP: scenario", Some(""), "WIP: scenario"),
        (true, " scenario", Some(", ,"), " scenario"),
    ] {
        let mut p = pr();
        p.draft = draft;
        p.title = source.into();
        let prefixes = configured.map(WipPrefixes::from_csv).unwrap_or_default();
        assert_eq!(title(&p, &prefixes), expected, "{source:?}");
        assert_eq!(pull_request(&p, &prefixes)["title"], expected);
    }
}
#[test]
fn action_status_and_conclusion_table() {
    for (state, description, expected_status, expected_conclusion) in [
        (S::Pending, "Waiting to run", CheckStatusState::Queued, None),
        (
            S::Pending,
            "Has started running",
            CheckStatusState::InProgress,
            None,
        ),
        (
            S::Success,
            "Successful in 2s",
            CheckStatusState::Completed,
            Some(CheckConclusionState::Success),
        ),
        (
            S::Failure,
            "Failed",
            CheckStatusState::Completed,
            Some(CheckConclusionState::Failure),
        ),
        (
            S::Error,
            "Error",
            CheckStatusState::Completed,
            Some(CheckConclusionState::Failure),
        ),
        (
            S::Warning,
            "Warning",
            CheckStatusState::Completed,
            Some(CheckConclusionState::Failure),
        ),
        (
            S::Success,
            "Has been skipped",
            CheckStatusState::Completed,
            Some(CheckConclusionState::Skipped),
        ),
    ] {
        let rows = mapped(&[status("ci / check (push)", state, description)]);
        let Check::Action {
            workflow,
            job,
            event,
            details_url,
            status,
            conclusion,
            ..
        } = &rows[0]
        else {
            panic!("expected Action")
        };
        assert_eq!(
            (workflow.as_str(), job.as_str(), event.as_str()),
            ("ci", "check", "push")
        );
        assert_eq!(
            details_url.as_str(),
            "https://forge.test/o/r/actions/runs/5/jobs/1"
        );
        assert_eq!(
            (*status, *conclusion),
            (expected_status, expected_conclusion)
        );
    }
}
#[test]
fn plain_status_and_action_detection_table() {
    for (source, expected) in [
        (S::Pending, StatusState::Pending),
        (S::Success, StatusState::Success),
        (S::Failure, StatusState::Failure),
        (S::Error, StatusState::Error),
        (S::Warning, StatusState::Failure),
    ] {
        let rows = mapped(&[status("external", source, "description")]);
        assert!(
            matches!(&rows[0],Check::Status{state,target_url:Some(url),..} if *state==expected && url.as_str()=="https://forge.test/o/r/actions/runs/5/jobs/1")
        );
    }
    for (context, target, action) in [
        (
            "ci / lint (push)",
            "https://forge.test/o/r/actions/runs/1",
            true,
        ),
        ("ci / lint (push)", "/o/r/actions/runs/1", true),
        (
            "ci / lint (push)",
            "https://other.test/o/r/actions/runs/1",
            false,
        ),
        ("ci / lint (push)", "/o/other/actions/runs/1", false),
        ("ci / lint (push)", "/o/r/status/1", false),
        ("lint", "/o/r/actions/runs/1", false),
        ("ci / lint ()", "/o/r/actions/runs/1", false),
    ] {
        let mut row = status(context, S::Success, "");
        row.target_url = target.into();
        assert_eq!(
            matches!(mapped(&[row])[0], Check::Action { .. }),
            action,
            "{context} {target}"
        );
    }
}
#[test]
fn latest_context_uses_updated_at_then_id_even_if_unsorted() {
    let mut old = status("ci / check (push)", S::Failure, "");
    let mut new = old.clone();
    new.state = S::Success;
    new.id = 2;
    old.updated_at = "2026-10-30T18:00:00Z".into();
    for rows in [
        vec![old.clone(), new.clone()],
        vec![new.clone(), old],
        vec![status("ci / check (push)", S::Failure, ""), new],
    ] {
        let result = mapped(&rows);
        assert_eq!(result.len(), 1);
        assert_eq!(rollup(&result), Some(StatusState::Success));
    }
}
#[test]
fn rollup_order_table() {
    for (states, expected) in [
        (vec![], None),
        (vec![S::Success], Some(StatusState::Success)),
        (vec![S::Success, S::Warning], Some(StatusState::Failure)),
        (vec![S::Pending, S::Success], Some(StatusState::Pending)),
        (vec![S::Pending, S::Failure], Some(StatusState::Failure)),
        (vec![S::Success, S::Error], Some(StatusState::Failure)),
    ] {
        let rows: Vec<_> = states
            .into_iter()
            .enumerate()
            .map(|(i, s)| status(&format!("external-{i}"), s, ""))
            .collect();
        assert_eq!(rollup(&mapped(&rows)), expected);
    }
    for conclusion in [
        CheckConclusionState::Failure,
        CheckConclusionState::TimedOut,
        CheckConclusionState::Cancelled,
        CheckConclusionState::ActionRequired,
    ] {
        let mut rows = mapped(&[status("ci / lint (push)", S::Success, "")]);
        if let Check::Action { conclusion: c, .. } = &mut rows[0] {
            *c = Some(conclusion);
        }
        assert_eq!(rollup(&rows), Some(StatusState::Failure));
    }
}
#[test]
fn required_context_glob_table() {
    let p = Protection {
        enable_status_check: true,
        status_check_contexts: vec!["ci / * (push)".into()],
        ..Default::default()
    };
    for (context, expected) in [
        ("ci / lint (push)", true),
        ("ci / check (push)", true),
        ("ci / lint (pull_request)", false),
        ("external", false),
    ] {
        assert_eq!(required(context, Some(&p)), expected);
        assert!(!required(context, None));
        assert!(!required(
            context,
            Some(&Protection {
                enable_status_check: false,
                ..p.clone()
            })
        ));
    }
}
#[test]
fn status_gate_matches_forgejo() {
    let gate = |contexts: &[&str]| Protection {
        enable_status_check: true,
        status_check_contexts: contexts.iter().map(|c| (*c).into()).collect(),
        ..Default::default()
    };
    let external = gate(&["external"]);
    let any = gate(&[]);
    for (protection, rows, passes) in [
        (None, vec![status("x", S::Failure, "")], true),
        (
            Some(&external),
            vec![status("external", S::Success, "")],
            true,
        ),
        (
            Some(&external),
            vec![status("external", S::Warning, "")],
            false,
        ),
        (
            Some(&external),
            vec![status("other", S::Success, "")],
            false,
        ),
        // Checks enabled with no pattern: every status counts, and none at all fails.
        (Some(&any), vec![], false),
        (Some(&any), vec![status("x", S::Success, "")], true),
        (Some(&any), vec![status("x", S::Failure, "")], false),
    ] {
        let mut rows = rows;
        for (id, row) in rows.iter_mut().enumerate() {
            row.id = id as i64;
        }
        assert_eq!(required_checks_pass(&mapped(&rows), protection), passes);
    }
    assert!(required("anything", Some(&any)));
}
#[test]
fn action_descriptions_and_subpath_links() {
    let action = |state, description: &str| {
        let row = status("ci / test (push)", state, description);
        match &checks(&[row], "o", "r", &"https://forge.test".parse().unwrap())[0] {
            Check::Action {
                status, conclusion, ..
            } => (*status, *conclusion),
            Check::Status { .. } => panic!("not an action"),
        }
    };
    use CheckConclusionState as C;
    use CheckStatusState as T;
    assert_eq!(
        action(S::Failure, "Has been cancelled"),
        (T::Completed, Some(C::Cancelled))
    );
    assert_eq!(
        action(S::Failure, "Failing after 3s"),
        (T::Completed, Some(C::Failure))
    );
    assert_eq!(
        action(S::Pending, "Blocked by required conditions"),
        (T::Waiting, None)
    );
    assert_eq!(action(S::Pending, "Waiting to run"), (T::Queued, None));
    assert_eq!(
        action(S::Success, "Has been skipped"),
        (T::Completed, Some(C::Skipped))
    );

    let mut row = status("ci / test (push)", S::Success, "Successful in 3s");
    row.target_url = "/forge/O/R/actions/runs/5/jobs/1".into();
    let public = "https://forge.test/forge/".parse().unwrap();
    assert!(matches!(
        checks(&[row], "o", "r", &public)[0],
        Check::Action { .. }
    ));
}
#[test]
fn review_state_and_latest_per_user_table() {
    for (source, dismissed, expected) in [
        ("APPROVED", false, PullRequestReviewState::Approved),
        (
            "REQUEST_CHANGES",
            false,
            PullRequestReviewState::ChangesRequested,
        ),
        ("COMMENT", false, PullRequestReviewState::Commented),
        ("PENDING", false, PullRequestReviewState::Pending),
        ("APPROVED", true, PullRequestReviewState::Dismissed),
    ] {
        let mut r = review(1, 1, source);
        r.dismissed = dismissed;
        assert_eq!(review_state(&r), expected);
    }
    let rows = vec![
        review(1, 1, "APPROVED"),
        review(2, 1, "COMMENT"),
        review(3, 1, "PENDING"),
        review(4, 2, "REQUEST_CHANGES"),
    ];
    assert_eq!(
        latest_reviews(&rows)
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [2, 4]
    );
}
#[test]
fn review_decision_counts_only_official_reviews_like_forgejo() {
    let p = Protection {
        required_approvals: 1,
        ..Default::default()
    };
    let official = |mut r: Review| {
        r.official = true;
        r
    };
    let approve = official(review(1, 1, "APPROVED"));
    let mut stale = approve.clone();
    stale.stale = true;
    let mut dismissed = approve.clone();
    dismissed.dismissed = true;
    let unofficial = review(1, 1, "APPROVED");
    let reject = official(review(2, 2, "REQUEST_CHANGES"));
    let unofficial_reject = review(2, 2, "REQUEST_CHANGES");
    let mut team_request = official(review(3, 0, "REQUEST_REVIEW"));
    team_request.user = None;
    let blocking = Protection {
        block_on_rejected_reviews: true,
        ..p.clone()
    };
    for (protection, reviews, expected) in [
        (None, vec![reject.clone()], None),
        (Some(Protection::default()), vec![reject.clone()], None),
        (
            Some(p.clone()),
            vec![],
            Some(PullRequestReviewDecision::ReviewRequired),
        ),
        (
            Some(p.clone()),
            vec![approve.clone()],
            Some(PullRequestReviewDecision::Approved),
        ),
        // Forgejo counts only official approvals, whitelist or not.
        (
            Some(p.clone()),
            vec![unofficial],
            Some(PullRequestReviewDecision::ReviewRequired),
        ),
        (
            Some(p.clone()),
            vec![dismissed],
            Some(PullRequestReviewDecision::ReviewRequired),
        ),
        // A rejection blocks only under block_on_rejected_reviews, and only when official.
        (
            Some(p.clone()),
            vec![approve.clone(), reject.clone()],
            Some(PullRequestReviewDecision::Approved),
        ),
        (
            Some(blocking.clone()),
            vec![approve.clone(), reject],
            Some(PullRequestReviewDecision::ChangesRequested),
        ),
        (
            Some(blocking),
            vec![approve.clone(), unofficial_reject],
            Some(PullRequestReviewDecision::Approved),
        ),
        // Stale approvals count unless ignore_stale_approvals.
        (
            Some(p.clone()),
            vec![stale.clone()],
            Some(PullRequestReviewDecision::Approved),
        ),
        (
            Some(Protection {
                ignore_stale_approvals: true,
                ..p.clone()
            }),
            vec![stale],
            Some(PullRequestReviewDecision::ReviewRequired),
        ),
        // An outstanding official request, even a team's, blocks under its flag alone.
        (
            Some(p.clone()),
            vec![approve.clone(), team_request.clone()],
            Some(PullRequestReviewDecision::Approved),
        ),
        (
            Some(Protection {
                block_on_official_review_requests: true,
                ..Default::default()
            }),
            vec![approve.clone(), team_request],
            Some(PullRequestReviewDecision::ReviewRequired),
        ),
        // A rule read from the branch summary hides the review flags: a passing count is
        // indeterminate, a failing one is not.
        (
            Some(Protection {
                partial: true,
                ..p.clone()
            }),
            vec![approve.clone()],
            None,
        ),
        (
            Some(Protection {
                partial: true,
                ..p.clone()
            }),
            vec![],
            Some(PullRequestReviewDecision::ReviewRequired),
        ),
    ] {
        assert_eq!(review_decision(&reviews, protection.as_ref()), expected);
    }
}
#[test]
fn merge_state_precedence_table() {
    let required = Protection {
        enable_status_check: true,
        status_check_contexts: vec!["external".into()],
        block_on_outdated_branch: true,
        ..Default::default()
    };
    // Forgejo refuses a merge that changes a protected file (`services/pull/merge.go:544-547`), and
    // the gateway can't tell which files those are.
    let files = Protection {
        protected_file_patterns: " ; *.lock".into(),
        ..Default::default()
    };
    let failing = mapped(&[status("external", S::Failure, "")]);
    let passing = mapped(&[status("external", S::Success, "")]);
    for (closed, conflict, draft, behind, checks, protection, decision, expected) in [
        (
            true,
            true,
            true,
            true,
            &failing[..],
            Some(&required),
            Some(PullRequestReviewDecision::ReviewRequired),
            MergeStateStatus::Unknown,
        ),
        (
            false,
            true,
            true,
            true,
            &failing[..],
            Some(&required),
            None,
            MergeStateStatus::Dirty,
        ),
        (
            false,
            false,
            true,
            true,
            &failing[..],
            Some(&required),
            None,
            MergeStateStatus::Behind,
        ),
        (
            false,
            false,
            false,
            true,
            &failing[..],
            Some(&required),
            None,
            MergeStateStatus::Behind,
        ),
        (
            false,
            false,
            false,
            false,
            &failing[..],
            Some(&required),
            None,
            MergeStateStatus::Blocked,
        ),
        (
            false,
            false,
            false,
            false,
            &[][..],
            Some(&required),
            None,
            MergeStateStatus::Blocked,
        ),
        (
            false,
            false,
            false,
            false,
            &passing[..],
            None,
            Some(PullRequestReviewDecision::ChangesRequested),
            MergeStateStatus::Blocked,
        ),
        (
            false,
            false,
            false,
            false,
            &failing[..],
            None,
            None,
            MergeStateStatus::Unstable,
        ),
        (
            false,
            false,
            false,
            false,
            &passing[..],
            Some(&required),
            None,
            MergeStateStatus::Clean,
        ),
        (
            false,
            false,
            true,
            false,
            &[][..],
            None,
            None,
            MergeStateStatus::Clean,
        ),
        (
            false,
            false,
            false,
            false,
            &passing[..],
            Some(&files),
            None,
            MergeStateStatus::Unknown,
        ),
        (
            false,
            false,
            false,
            false,
            &passing[..],
            Some(&files),
            Some(PullRequestReviewDecision::ChangesRequested),
            MergeStateStatus::Blocked,
        ),
    ] {
        let mut p = pr();
        if closed {
            p.state = "closed".into();
        }
        p.mergeable = !conflict;
        p.draft = draft;
        assert_eq!(
            merge_state(
                &p,
                if conflict {
                    MergeableState::Conflicting
                } else {
                    MergeableState::Mergeable
                },
                checks,
                protection,
                decision,
                behind
            ),
            expected
        );
    }
}

#[test]
fn review_requests_missing_empty_and_populated_table() {
    for count in [None, Some(0), Some(2)] {
        let mut p = pr();
        p.requested_reviewers = count.map(|n| {
            (1..=n)
                .map(|id| review(id, id, "COMMENT").user.unwrap())
                .collect()
        });
        assert_eq!(review_requests(&p).len(), count.unwrap_or(0) as usize);
    }
}

#[test]
fn unknown_mergeability_precedes_draft_and_protection_rules() {
    let mut p = pr();
    p.draft = true;
    assert_eq!(
        merge_state(&p, MergeableState::Unknown, &[], None, None, true),
        MergeStateStatus::Unknown
    );
}

#[test]
fn captured_forgejo15_response_uses_status_not_state() {
    let row: CommitStatus =
        serde_json::from_str(include_str!("../../../../fixtures/forgejo15-status.json")).unwrap();
    assert!(matches!(row.state, S::Failure));
    assert!(matches!(
        checks(
            &[row],
            "dirtydishes",
            "gfj-oracle",
            &"https://forge.test".parse().unwrap()
        )[0],
        Check::Action { .. }
    ));
}

#[test]
fn review_requests_keep_users_and_teams() {
    let mut p = pr();
    p.requested_reviewers = Some(vec![review(1, 1, "COMMENT").user.unwrap()]);
    p.requested_reviewers_teams = Some(vec![crate::forgejo::RequestedTeam {
        name: "reviewers".into(),
        organization: crate::forgejo::TeamOrganization {
            id: 2,
            name: "org".into(),
        },
    }]);
    let reviewers = review_requests(&p);
    assert!(matches!(&reviewers[0],Reviewer::User(u) if u.id==1));
    assert!(
        matches!(&reviewers[1],Reviewer::Team(t) if t.name=="reviewers" && t.organization.name=="org")
    );
}
#[test]
fn deleted_head_branches_keep_their_name() {
    let pr: PullRequest = serde_json::from_value(json!({"number":1,"title":"t","state":"closed","merged":true,"draft":false,"html_url":"","head":{"label":"topic","ref":"refs/pull/1/head","sha":"abc","repo":null},"base":{"label":"main","ref":"main","sha":"def","repo":null},"user":{"id":1,"login":"o","avatar_url":""},"merged_at":null})).unwrap();
    assert_eq!(pr.head.name(), "topic");
    assert_eq!(
        pull_request(&pr, &WipPrefixes::default())["head"]["ref"],
        "topic"
    );
}

/// PR web URLs take GitHub's `/pull/{n}` layout, and nothing else in the URL changes (MAPPING §8).
#[test]
fn pull_web_urls_take_githubs_layout() {
    for (forgejo, github) in [
        (
            "https://forge.test/o/r/pulls/1",
            "https://forge.test/o/r/pull/1",
        ),
        (
            "https://forge.test/o/r/pulls/12/files#issuecomment-7",
            "https://forge.test/o/r/pull/12/files#issuecomment-7",
        ),
        (
            "https://forge.test/o/r/pulls/3?tab=x#y",
            "https://forge.test/o/r/pull/3?tab=x#y",
        ),
        // A forge under a subpath, with a port, keeps both.
        (
            "http://h:3000/git/o/r/pulls/4",
            "http://h:3000/git/o/r/pull/4",
        ),
        // A repository or owner named `pulls` is a name, not the route.
        (
            "https://forge.test/o/pulls/pulls/5",
            "https://forge.test/o/pulls/pull/5",
        ),
        (
            "https://forge.test/pulls/9/pulls/6",
            "https://forge.test/pulls/9/pull/6",
        ),
        (
            "https://forge.test/o/r/pulls/2/commits/abc",
            "https://forge.test/o/r/pull/2/commits/abc",
        ),
        (
            "https://forge.test/o/r/pulls/2/",
            "https://forge.test/o/r/pull/2/",
        ),
    ] {
        assert_eq!(pull_web_url(forgejo), github, "{forgejo}");
    }
    for unchanged in [
        "https://forge.test/o/r/issues/1",
        "https://forge.test/o/r/pulls",
        "https://forge.test/o/r/pulls/new",
        "https://forge.test/o/r",
        "https://forge.test/pulls/1",
        "https://forge.test/o/r/commit/abc#pulls/1",
        // A query before any path, and an owner `pulls` with a numeric repository under a subpath.
        "https://forge.test?x/o/r/pulls/1",
        "https://forge.test/a/b/pulls/5/issues/3",
        "https://forge.test/o/r/pulls/5/unknown",
        "not a url",
        "",
    ] {
        assert_eq!(pull_web_url(unchanged), unchanged);
    }
    assert_eq!(
        pull_request(&pr(), &WipPrefixes::default())["html_url"],
        "https://forge.test/o/r/pull/1"
    );
    assert_eq!(
        forgejo_pull_web_url("https://forge.test/o/r/pull/12/files#issuecomment-7"),
        "https://forge.test/o/r/pulls/12/files#issuecomment-7"
    );
    assert_eq!(
        forgejo_pull_web_url("https://forge.test/o/r/compare/main...topic"),
        "https://forge.test/o/r/compare/main...topic"
    );
}

/// Bodies render as GitHub-flavoured Markdown, as GitHub's bodyHTML does.
#[test]
fn body_html_renders_github_flavoured_markdown() {
    let cases = [
        ("", ""),
        ("  \r\n ", ""),
        ("line one\r\nline two", "<p>line one<br>\nline two</p>"),
        (
            "# Title\n\n- a\n- b",
            "<h1>Title</h1>\n<ul>\n<li>a</li>\n<li>b</li>\n</ul>",
        ),
        (
            "| a | b |\n|---|:-:|\n| 1 | 2 |",
            "<table>\n<thead>\n<tr>\n<th>a</th>\n<th align=\"center\">b</th>\n</tr>\n</thead>\n\
             <tbody>\n<tr>\n<td>1</td>\n<td align=\"center\">2</td>\n</tr>\n</tbody>\n</table>",
        ),
        (
            "- [x] done\n- [ ] todo",
            "<ul>\n<li><input checked=\"\" disabled=\"\" type=\"checkbox\"> done</li>\n\
             <li><input disabled=\"\" type=\"checkbox\"> todo</li>\n</ul>",
        ),
        ("~~gone~~", "<p><del>gone</del></p>"),
        (
            "see https://example.com/x and www.example.com",
            "<p>see <a href=\"https://example.com/x\" rel=\"nofollow noopener noreferrer\">\
             https://example.com/x</a> and <a href=\"http://www.example.com\" \
             rel=\"nofollow noopener noreferrer\">www.example.com</a></p>",
        ),
        (
            "```rust\nfn main() {}\n```",
            "<pre lang=\"rust\"><code>fn main() {}\n</code></pre>",
        ),
        (
            "<details><summary>s</summary>body</details>",
            "<details><summary>s</summary>body</details>",
        ),
        ("a & \"b\" <c>", "<p>a &amp; \"b\" </p>"),
    ];
    for (body, html) in cases {
        assert_eq!(body_html(body), html, "{body:?}");
    }
}

/// Clients trust bodyHTML as sanitized, so markup that could run script, restyle the page, submit
/// a form or load a non-HTTP resource never comes through, whatever the body holds.
#[test]
fn body_html_removes_hostile_markup() {
    let rel = "rel=\"nofollow noopener noreferrer\"";
    let cases = [
        ("a <script>alert(1)</script> b", "<p>a  b</p>".to_owned()),
        ("<img src=x onerror=alert(1)>", "<img>".to_owned()),
        ("<img src=\"/attachments/a.png\">", "<img>".to_owned()),
        (
            "![i](javascript:alert(1))",
            "<p><img alt=\"i\"></p>".to_owned(),
        ),
        (
            "<img src=\"https://forge.example/a.png\" alt=a width=10>",
            "<img src=\"https://forge.example/a.png\" alt=\"a\" width=\"10\">".to_owned(),
        ),
        ("[x](javascript:alert(1))", format!("<p><a {rel}>x</a></p>")),
        ("[x](JaVaScRiPt:alert(1))", format!("<p><a {rel}>x</a></p>")),
        (
            "[x](data:text/html;base64,PHNjcmlwdD4=)",
            format!("<p><a {rel}>x</a></p>"),
        ),
        (
            "<a href=\"vbscript:x\">x</a>",
            format!("<p><a {rel}>x</a></p>"),
        ),
        (
            "<a href=\"&#106;avascript:alert(1)\">x</a>",
            format!("<p><a {rel}>x</a></p>"),
        ),
        (
            "<a href=\"java&#x09;script:alert(1)\">x</a>",
            format!("<p><a {rel}>x</a></p>"),
        ),
        (
            "<a href=\" \u{1}javascript:alert(1)\">x</a>",
            format!("<p><a {rel}>x</a></p>"),
        ),
        (
            "<a href=\"javascript&colon;alert(1)\">x</a>",
            format!("<p><a {rel}>x</a></p>"),
        ),
        (
            "<blockquote cite=\"javascript:alert(1)\">q</blockquote>",
            "<blockquote>q</blockquote>".to_owned(),
        ),
        (
            "[ok](https://example.com) [mail](mailto:a@example.com) [rel](../x)",
            format!(
                "<p><a href=\"https://example.com\" {rel}>ok</a> \
                 <a href=\"mailto:a@example.com\" {rel}>mail</a> <a href=\"../x\" {rel}>rel</a></p>"
            ),
        ),
        (
            "<a href=\"https://e.com\" onclick=\"x()\" style=\"color:red\" target=_blank id=x class=c>l</a>",
            format!("<p><a href=\"https://e.com\" {rel}>l</a></p>"),
        ),
        (
            "<div><b>unclosed <i>tags",
            "<div><b>unclosed <i>tags\n</i></b></div>".to_owned(),
        ),
        (
            "<!-- <script>alert(1)</script> --> after",
            "after".to_owned(),
        ),
        ("<!-- <a href=x>hidden", "".to_owned()),
        (
            "<input type=text value=x>",
            "<input type=\"checkbox\">".to_owned(),
        ),
        ("<form action=/x><button>b</button></form>", "b".to_owned()),
        (
            "<iframe src=https://e.com></iframe><object data=x></object><embed src=x><style>p{}</style>",
            "".to_owned(),
        ),
        (
            "<svg><script>alert(1)</script></svg><math><mi>x</mi></math>",
            "<p></p>".to_owned(),
        ),
        (
            "<p style=\"background:url(x)\">s</p>",
            "<p>s</p>".to_owned(),
        ),
        (
            "<base href=https://evil.example><meta http-equiv=refresh content=0>",
            "".to_owned(),
        ),
    ];
    for (body, html) in cases {
        assert_eq!(body_html(body), html, "{body:?}");
    }
}

/// A body built to make the renderer or the sanitizer slow, or one too long to render, comes back
/// quickly, as escaped paragraphs or with its raw HTML shown as text.
#[test]
fn body_html_stays_fast_on_pathological_bodies() {
    let deep_quotes = ">".repeat(60_000);
    // 100 levels in under 64 KiB, so the depth guard, not the size cap, decides.
    let deep_lists: String = (0..100)
        .map(|i| format!("{}- x\n", "  ".repeat(i)))
        .collect();
    let ampersands = "&".repeat(65_000);
    let deep_html = "<div>".repeat(12_000);
    let raw_then_table = "<div>".repeat(200) + "\n\n|a|b|\n|-|-|\n" + &"|x|y|\n".repeat(9_000);
    let huge = "word ".repeat(1_000_000);
    for body in [
        &deep_quotes,
        &deep_lists,
        &deep_html,
        &raw_then_table,
        &ampersands,
        &huge,
    ] {
        let started = std::time::Instant::now();
        let html = body_html(body);
        // Generous, so a slow CI machine passes; the unguarded cases took seconds to minutes.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{} bytes took {:?}",
            body.len(),
            started.elapsed()
        );
        assert!(!html.contains("<script"), "{html:.80}");
    }
    // Past the guards the body still reads, escaped.
    assert!(body_html(&deep_quotes).starts_with("<p>&gt;&gt;&gt;"));
    assert!(
        body_html(&deep_lists).starts_with("<p>- x"),
        "{:.80}",
        body_html(&deep_lists)
    );
    assert!(body_html(&ampersands).starts_with("<p>&amp;&amp;"));
    assert!(body_html(&huge).starts_with("<p>word word"));
    // Too much raw HTML: shown as text.
    assert!(
        body_html(&deep_html).starts_with("&lt;div&gt;&lt;div&gt;"),
        "{:.80}",
        body_html(&deep_html)
    );
}

/// REST's combined status holds the plain statuses and GitHub's state over them; Actions jobs are
/// check runs, with the states `statusCheckRollup` gives them.
#[test]
fn commit_status_and_check_runs_split_like_github() {
    let mut external = status("lint", S::Failure, "lint failed");
    external.id = 2;
    external.target_url = "https://ci.example/build/7".into();
    let rows = [
        status("ci / check (push)", S::Success, "Successful in 3s"),
        status("ci / slow (push)", S::Pending, "Waiting to run"),
        external,
    ];
    let checks = mapped(&rows);
    let combined = combined_status(&checks, "abc");
    assert_eq!(combined["state"], "failure");
    assert_eq!(combined["total_count"], 1);
    assert_eq!(
        combined["statuses"],
        json!([{"id":2,"context":"lint","state":"failure","description":"lint failed",
            "target_url":"https://ci.example/build/7",
            "created_at":"2026-10-30T19:00:00Z","updated_at":"2026-10-30T19:00:02Z"}])
    );
    let runs = check_runs(&checks, "abc");
    assert_eq!(runs["total_count"], 2);
    let summary: Vec<_> = runs["check_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            json!([
                r["name"],
                r["status"],
                r["conclusion"],
                r["head_sha"],
                r["completed_at"].is_null()
            ])
        })
        .collect();
    assert_eq!(
        summary,
        [
            json!(["check", "completed", "success", "abc", false]),
            json!(["slow", "queued", null, "abc", true])
        ]
    );
    assert_eq!(
        runs["check_runs"][0]["details_url"],
        "https://forge.test/o/r/actions/runs/5/jobs/1"
    );
    // GitHub's combined state: no statuses is pending, all passing is success.
    assert_eq!(combined_status(&[], "abc")["state"], "pending");
    let passing = mapped(&[status("lint", S::Success, "ok")]);
    assert_eq!(combined_status(&passing, "abc")["state"], "success");
    // Any pending status keeps it pending; an error counts as a failure.
    let mut waiting = status("deploy", S::Pending, "waiting");
    waiting.id = 3;
    let pending = mapped(&[status("lint", S::Success, "ok"), waiting]);
    assert_eq!(combined_status(&pending, "abc")["state"], "pending");
    let errored = mapped(&[status("lint", S::Error, "broke")]);
    assert_eq!(combined_status(&errored, "abc")["state"], "failure");
}

/// Checks come out in context order whatever order Forgejo lists them in, so gh's newest-first sort
/// breaks a tie the same way every time (bead 1u7).
#[test]
fn checks_keep_context_order_whatever_forgejo_sends() {
    // The alphabetically first context has the higher id, so neither id order nor input order
    // gives the expected result: only context order does.
    let lint = status("ci / lint (push)", S::Success, "Successful in 1s");
    let mut check = status("ci / check (push)", S::Success, "Successful in 1s");
    check.id = 2;
    let names = |rows: &[CommitStatus]| -> Vec<String> {
        mapped(rows)
            .iter()
            .map(|c| c.source().context.clone())
            .collect()
    };
    let expected = ["ci / check (push)", "ci / lint (push)"];
    assert_eq!(names(&[lint.clone(), check.clone()]), expected);
    assert_eq!(names(&[check, lint]), expected);
}

/// An image loads by itself in the viewer's client, so only the forge's own images stay: an image
/// anywhere else could be the client's own API, a service on the viewer's network or a tracker. A
/// scheme without `//` resolves against the page showing it. Kept sources come back as parsed.
#[test]
fn body_html_images_need_an_absolute_http_url() {
    for src in [
        "https:/api/logout",
        "https:api/logout",
        "https:\\\\api\\\\logout",
        "https&#58;/api/logout",
        "https:&#x2F;api/logout",
        "http:/x",
        "//evil.example/x",
        "\\\\evil.example\\x",
        "javascript:alert(1)",
        "data:image/png;base64,AAAA",
        "http://localhost:8080/admin",
        "http://app.localhost/x",
        "http://printer.local/x",
        "http://127.0.0.1/x",
        "http://10.0.0.1/x",
        "http://192.168.1.1/x",
        "http://169.254.169.254/latest/meta-data",
        "http://[::1]/x",
        "http://[fd00::1]/x",
        "http://[::ffff:127.0.0.1]/x",
        "http://0.0.0.0/x",
        // Another origin, even an ordinary one: the client's own API, or a tracker.
        "https://chatgpt.com/backend-api/me",
        "https://example.com/i.png",
        "https:///x",
        "http://forge.example/i.png",
        "https://forge.example:8443/i.png",
        "https://forge.example.evil.example/i.png",
    ] {
        assert_eq!(body_html(&format!("<img src=\"{src}\">")), "<img>", "{src}");
    }
    // The forge's own images stay, in parsed form.
    assert_eq!(
        body_html("<img src=\" HTTPS://Forge.Example/a b.png\">"),
        "<img src=\"https://forge.example/a%20b.png\">"
    );
    assert_eq!(
        body_html("![a](https://forge.example/attachments/1)"),
        "<p><img src=\"https://forge.example/attachments/1\" alt=\"a\"></p>"
    );
    // Without a forge, no image stays.
    assert_eq!(
        super::body_html("![a](https://forge.example/i.png)", None),
        "<p><img alt=\"a\"></p>"
    );
}

/// A table's short rows are filled out to the header's width, so a few kilobytes of pipes could
/// ask for millions of cells. Past the bound, the pipes read as text, and the body stays small and
/// quick.
#[test]
fn body_html_bounds_sparse_tables() {
    let table = format!(
        "{}\n{}|\n{}\n",
        "|".repeat(708),
        "|-".repeat(707),
        "x\n".repeat(708)
    );
    for body in [table.repeat(2), table.repeat(9)] {
        let started = std::time::Instant::now();
        let html = body_html(&body);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(html.len() < 4 * body.len(), "{} bytes of HTML", html.len());
        assert!(!html.contains("<table>"));
    }
    // An ordinary table still renders.
    let ordinary = "|a|b|c|\n|-|-|-|\n".to_owned() + &"|1|2|3|\n".repeat(500);
    assert!(body_html(&ordinary).starts_with("<table>"));
}

/// A table under the cell bound can still be a hundred times larger as HTML than as Markdown, and a
/// document can hold many of them. Once the shared budget runs out, the rest come back escaped
/// without being parsed.
#[test]
fn body_html_budget_bounds_repeated_sparse_tables() {
    let table = format!(
        "{}\n{}|\n{}",
        "|".repeat(100),
        "|-".repeat(99),
        "x\n".repeat(100)
    );
    let forge = "https://forge.example".parse().unwrap();
    let budget = std::sync::atomic::AtomicUsize::new(1024 * 1024);
    let started = std::time::Instant::now();
    let rendered = (0..100)
        .map(|_| super::body_html_within(&table, Some(&forge), &budget))
        .filter(|html| html.starts_with("<table>"))
        .count();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!((1..=12).contains(&rendered), "{rendered} tables rendered");
    assert_eq!(budget.load(std::sync::atomic::Ordering::Relaxed), 0);
}
