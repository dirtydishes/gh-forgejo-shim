//! Forgejo's glob dialect, gobwas/glob v0.2.3 (`syntax/lexer/lexer.go`, `syntax/ast/parser.go`),
//! translated to an anchored regex so matching stays linear in the input.
//!
//! Checked against Forgejo's own matching on 200,000 random cases. It agrees on every non-empty
//! ASCII name for patterns with balanced, non-empty groups and no run of three `*`. gobwas
//! optimizer bugs outside that range (`?` matching an empty string, `{}` groups, `?` on multibyte
//! characters after other tokens) are not reproduced.
use regex::Regex;
use std::{iter::Peekable, str::Chars};

/// gobwas's meta characters (`syntax/lexer/lexer.go` `specials`), which Forgejo also uses to
/// tell a plain branch-rule name from a glob.
pub const SYNTAX: &[char] = &['*', '?', '\\', '[', ']', '{', '}'];

/// `pattern` as gobwas compiles it, or `None` where gobwas reports a syntax error. `*` and `?`
/// never match `separator`; `**` matches anything.
pub fn compile(pattern: &str, separator: Option<char>) -> Option<Regex> {
    // gobwas compiles exactly `prefix**suffix` to a prefix test and a suffix test that may
    // overlap, so `a/**/b` matches `a/b` (`compiler/compiler.go:73-76`, `match/prefix_suffix.go`).
    if let Some((prefix, suffix)) = pattern.split_once("**")
        && ![prefix, suffix]
            .iter()
            .any(|t| t.is_empty() || t.contains(SYNTAX))
    {
        let mut re = format!(
            r"\A(?:{}(?s:.*){}",
            regex::escape(prefix),
            regex::escape(suffix)
        );
        for k in 1..=prefix.len().min(suffix.len()) {
            if suffix.is_char_boundary(k) && prefix.ends_with(&suffix[..k]) {
                re.push('|');
                re.push_str(&regex::escape(&format!("{prefix}{}", &suffix[k..])));
            }
        }
        re.push_str(r")\z");
        return Regex::new(&re).ok();
    }
    let (any, one) = match separator {
        Some(sep) => (format!("[^{}]*", hex(sep)), format!("[^{}]", hex(sep))),
        None => ("(?s:.*)".to_owned(), "(?s:.)".to_owned()),
    };
    let mut re = String::from(r"\A(?:");
    let mut chars = pattern.chars().peekable();
    let mut depth = 0;
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.next_if_eq(&'*').is_some() => re.push_str("(?s:.*)"),
            '*' => re.push_str(&any),
            '?' => re.push_str(&one),
            '[' => class(&mut chars, &mut re)?,
            '{' => {
                depth += 1;
                re.push_str("(?:");
            }
            ',' if depth > 0 => re.push('|'),
            '}' if depth > 0 => {
                depth -= 1;
                re.push(')');
            }
            // A trailing backslash escapes nothing and is dropped.
            '\\' => re.extend(chars.next().map(hex)),
            c => re.push_str(&hex(c)),
        }
    }
    // gobwas accepts an unclosed `{`, ending its alternatives at the end of the pattern.
    re.extend(std::iter::repeat_n(')', depth));
    re.push_str(r")\z");
    Regex::new(&re).ok()
}

/// The `[...]` after its `[`: an optional `!`, then one `lo-hi` range or a list of characters.
/// The range bounds are taken raw; the list honors backslash escapes.
fn class(chars: &mut Peekable<Chars>, re: &mut String) -> Option<()> {
    let not = chars.next_if_eq(&'!').is_some();
    let first = chars.next()?;
    let mut set = String::new();
    if chars.next_if_eq(&'-').is_some() {
        let hi = chars.next()?;
        if hi < first || chars.next()? != ']' {
            return None;
        }
        set = format!("{}-{}", hex(first), hex(hi));
    } else {
        let mut c = first;
        while c != ']' {
            set.push_str(&hex(if c == '\\' { chars.next()? } else { c }));
            c = chars.next()?;
        }
        if set.is_empty() {
            return None;
        }
    }
    re.push_str(&format!("[{}{set}]", if not { "^" } else { "" }));
    Some(())
}

fn hex(c: char) -> String {
    format!("\\x{{{:x}}}", u32::from(c))
}
