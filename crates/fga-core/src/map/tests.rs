use super::*;
use crate::forgejo::{CommitStatus, CommitStatusState as S, Protection, PullRequest, Review};
use serde_json::json;

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
        "not a url",
        "",
    ] {
        assert_eq!(pull_web_url(unchanged), unchanged);
    }
    assert_eq!(
        pull_request(&pr(), &WipPrefixes::default())["html_url"],
        "https://forge.test/o/r/pull/1"
    );
}
