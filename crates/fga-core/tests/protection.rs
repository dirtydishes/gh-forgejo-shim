use fga_core::forgejo::Protection;

fn rule(name: &str) -> Protection {
    Protection {
        rule_name: name.into(),
        ..Protection::default()
    }
}

fn chosen(rules: &[&str], branch: &str) -> Option<String> {
    Protection::for_branch(rules.iter().map(|name| rule(name)).collect(), branch)
        .map(|rule| rule.rule_name)
}

/// Forgejo v15 `models/git/protected_branch.go:81-107` and `protected_branch_list.go:18-44`.
#[test]
fn branch_rules_follow_forgejo_matching_and_order() {
    // Plain names match case-insensitively; globs keep `*` inside one path segment.
    assert_eq!(chosen(&["Main"], "main").as_deref(), Some("Main"));
    assert_eq!(
        chosen(&["release/*"], "release/a").as_deref(),
        Some("release/*")
    );
    assert_eq!(chosen(&["release/*"], "release/a/b"), None);
    assert_eq!(
        chosen(&["release/**"], "release/a/b").as_deref(),
        Some("release/**")
    );
    assert_eq!(chosen(&["{dev,qa}"], "qa").as_deref(), Some("{dev,qa}"));
    // Globs are case-sensitive.
    assert_eq!(chosen(&["Rel*"], "release"), None);
    // Plain names win over earlier globs; otherwise the first (oldest) rule wins.
    assert_eq!(chosen(&["ma*", "main"], "main").as_deref(), Some("main"));
    assert_eq!(chosen(&["m*", "ma*"], "main").as_deref(), Some("m*"));
    // An invalid glob matches only its literal text.
    assert_eq!(chosen(&["feat[x"], "feat[x").as_deref(), Some("feat[x"));
    assert_eq!(chosen(&["feat[x"], "featx"), None);
}

/// `services/pull/commit_status.go:28-41`: contexts are globs without separators, and an
/// invalid pattern never matches.
#[test]
fn required_contexts_are_separator_free_globs() {
    let protection = Protection {
        enable_status_check: true,
        status_check_contexts: vec!["ci/*".into(), "[".into()],
        ..Protection::default()
    };
    let required = |context: &str| fga_core::map::required(context, Some(&protection));
    assert!(required("ci/check (pull_request)"));
    assert!(required("ci/a/b"));
    assert!(!required("lint"));
    assert!(!required("["));
    let disabled = Protection {
        enable_status_check: false,
        ..protection.clone()
    };
    assert!(!fga_core::map::required("ci/a", Some(&disabled)));
}

/// Results from Forgejo v15's own matching (`ProtectedBranch.Match` and the required-context
/// check over gobwas/glob v0.2.3), run in Go. `None` marks a pattern gobwas rejects: a branch rule
/// then matches only its literal text, and a required context never matches.
#[test]
fn globs_match_like_forgejo() {
    #[rustfmt::skip]
    let cases = [
        // (pattern, name, branch rule matches, required context matches)
        ("release**", "release/a", Some(true), Some(true)),
        ("**/main", "main", Some(false), Some(false)),
        ("*", "a/b", Some(false), Some(true)),
        ("feat/?", "feat//", Some(false), Some(true)),
        ("feat/?", "feat/é", Some(true), Some(true)),
        ("[!a]", "/", Some(true), Some(true)),
        // gobwas tests `prefix**suffix` as a prefix and a suffix, which may overlap.
        ("release/**/stable", "release/stable", Some(true), Some(true)),
        ("ci/**/test", "ci/test", Some(true), Some(true)),
        ("{,release/}main", "main", Some(true), Some(true)),
        ("{a,{b,c}d}", "cd", Some(true), Some(true)),
        ("{a,b", "b", Some(true), Some(true)),
        ("rel\\*", "rel*", Some(true), Some(true)),
        ("rel\\*", "relx", Some(false), Some(false)),
        ("[ab-]", "-", Some(true), Some(true)),
        ("[\\]]", "]", Some(true), Some(true)),
        ("[a-b-c]", "a", None, None),
        ("[z-a]", "m", None, None),
        ("[]", "]", None, None),
    ];
    for (pattern, name, branch, context) in cases {
        let rule = rule(pattern);
        assert_eq!(
            rule.matches_branch(name),
            branch.unwrap_or(pattern == name),
            "branch rule {pattern:?} against {name:?}"
        );
        let protection = Protection {
            enable_status_check: true,
            status_check_contexts: vec![pattern.into()],
            ..Protection::default()
        };
        assert_eq!(
            fga_core::map::required(name, Some(&protection)),
            context.unwrap_or(false),
            "required context {pattern:?} against {name:?}"
        );
    }
}

/// Plain rule names compare under Go's `strings.EqualFold`, results again from Go.
#[test]
fn plain_names_fold_case_like_go() {
    for (rule_name, branch, matches) in [
        ("MÄIN", "mäin", true),
        ("ſ", "S", true),
        ("\u{212a}", "k", true),
        ("ẞ", "ß", true),
        ("ı", "i", false),
    ] {
        assert_eq!(
            rule(rule_name).matches_branch(branch),
            matches,
            "{rule_name:?}"
        );
    }
}
