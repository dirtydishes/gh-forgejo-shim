//! Where a review comment sits in the diff, from what Forgejo gives.
use crate::forgejo::{Review, ReviewComment};

/// Whether a review is one GitHub would list: not a review request, which Forgejo lists among
/// reviews, and not someone else's pending review.
pub fn listed(review: &Review, viewer: &str) -> bool {
    review.state != "REQUEST_REVIEW"
        && (review.state != "PENDING"
            || review
                .user
                .as_ref()
                .is_some_and(|user| user.login.eq_ignore_ascii_case(viewer)))
}

/// The commented line as the reviewer saw it: the last line of the comment's diff hunk, which
/// Forgejo cuts to end there (`CutDiffAroundLine`, Forgejo v15.0.0 `modules/git/diff.go`). Without
/// a hunk (Forgejo's `CODE_COMMENT_LINES = 0`, or one that can't be read) a new-side line is
/// unknown: its `position` is the line in the commit that last changed it (`LineBlame`), which can
/// be another line entirely. An old-side position is never blamed, so it stands.
pub fn commented_line(source: &ReviewComment) -> Option<i32> {
    // Forgejo blames only new-side lines, so an old-side position is the line the reviewer chose.
    let old = source.line() < 0;
    let line = hunk_end(&source.diff_hunk, old).or(old.then_some(source.original_position))?;
    i32::try_from(line).ok().filter(|line| *line != 0)
}

/// The number, on the old or new side, of a diff hunk's last line.
fn hunk_end(hunk: &str, old: bool) -> Option<u64> {
    let mut lines = hunk.lines();
    let header = lines.next()?.strip_prefix("@@ -")?;
    let (old_start, rest) = header.split_once(" +")?;
    let new_start = rest.split_once(' ')?.0;
    let start = |range: &str| range.split(',').next()?.parse::<u64>().ok();
    let (mut old_line, mut new_line) = (start(old_start)?, start(new_start)?);
    let mut last = None;
    for line in lines {
        match line.as_bytes().first() {
            Some(b'+') => {
                last = (!old).then_some(new_line);
                new_line += 1;
            }
            Some(b'-') => {
                last = old.then_some(old_line);
                old_line += 1;
            }
            Some(b'\\') => {}
            _ => {
                last = Some(if old { old_line } else { new_line });
                old_line += 1;
                new_line += 1;
            }
        }
    }
    last
}

/// At most this many closing references are read from one body.
const MAX_CLOSING_REFERENCES: usize = 50;

/// The issues a pull request's body says it closes, as `(repository, number)` with `None` for the
/// PR's own repository, first mention first. These are Forgejo's closing keywords, which it acts
/// on when the PR merges (`modules/references/references.go`, `CloseKeywords`).
pub fn closing_references(body: &str) -> Vec<(Option<(String, String)>, i32)> {
    static KEYWORD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)(?:^|[\s(\[])(?:close[sd]?|fix(?:es|ed)?|resolve[sd]?):?\s+(?:([\w.-]+)/([\w.-]+))?#(\d+)\b",
        )
        .expect("valid pattern")
    });
    let mut found: Vec<(Option<(String, String)>, i32)> = Vec::new();
    for captures in KEYWORD.captures_iter(body) {
        let Ok(number) = captures[3].parse::<i32>() else {
            continue;
        };
        let repository = captures
            .get(1)
            .zip(captures.get(2))
            .map(|(owner, repo)| (owner.as_str().to_owned(), repo.as_str().to_owned()));
        if !found.contains(&(repository.clone(), number)) {
            found.push((repository, number));
        }
        if found.len() == MAX_CLOSING_REFERENCES {
            break;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_references_follow_forgejos_keywords() {
        assert_eq!(
            closing_references("Fixes #12, and closes o/r#3.\nResolved: #12\nsee #4 (fix #5)"),
            vec![(None, 12), (Some(("o".into(), "r".into())), 3), (None, 5)]
        );
        assert!(closing_references("prefix#1 unfixes #2 fix#3").is_empty());
    }

    fn comment(diff_hunk: &str, position: u64, original_position: u64) -> ReviewComment {
        serde_json::from_value(
            serde_json::json!({"id":1,"body":"","user":null,"resolver":null,
            "pull_request_review_id":1,"created_at":"","path":"a.rs","diff_hunk":diff_hunk,
            "position":position,"original_position":original_position,"html_url":""}),
        )
        .unwrap()
    }

    /// Forgejo blames a comment's line to the commit that last changed it, so its `position` is
    /// no stand-in for the line the reviewer saw: without a hunk, the line is unknown.
    #[test]
    fn a_comment_without_a_hunk_has_no_line() {
        assert_eq!(
            commented_line(&comment("@@ -5,2 +5,3 @@\n a\n+b\n c", 3, 0)),
            Some(7)
        );
        assert_eq!(
            commented_line(&comment("@@ -5,1 +5,0 @@\n-x", 0, 5)),
            Some(5)
        );
        assert_eq!(commented_line(&comment("", 3, 0)), None);
        assert_eq!(commented_line(&comment("not a hunk", 3, 0)), None);
        // Forgejo blames only new-side lines: an old-side position is the reviewer's own line.
        assert_eq!(commented_line(&comment("", 0, 5)), Some(5));
    }
}
