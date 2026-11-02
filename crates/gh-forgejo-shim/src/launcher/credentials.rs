//! The launcher's git credential helper (launcher spec §3, "Git's credentials"): every git in a
//! launched call asks the run directory's `gh` script, which runs the wrapper with
//! `FJ_SHIM_CREDENTIALS` naming the `credentials` file. The wrapper answers from it alone.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, Write};
use std::path::Path;

use crate::launcher::ghconfig::credential_for;

/// The variable the run directory's `gh` script sets: the `credentials` file's path.
pub const CREDENTIALS_VAR: &str = "FJ_SHIM_CREDENTIALS";

/// The username gh's own helper gives git for a token.
const TOKEN_USER: &str = "x-access-token";

/// Answers `auth git-credential <op>` when `FJ_SHIM_CREDENTIALS` is set, before any guard or the
/// §7 revert, never routing or launching. `args` are gh's arguments, after `gh`. `None` for any
/// other call, which then routes as usual.
pub fn answer(
    credentials: Option<&OsStr>,
    args: &[OsString],
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    errors: &mut dyn Write,
) -> Option<i32> {
    let path = credentials.filter(|path| !path.is_empty())?;
    let [auth, command, operation] = args else {
        return None;
    };
    if auth != "auth" || command != "git-credential" {
        return None;
    }
    Some(match operation.to_str() {
        // As gh's helper: pretend to store and erase, since the token is the launcher's.
        Some("store" | "erase") => 0,
        Some("get") => get(Path::new(path), input, output),
        _ => {
            let _ = writeln!(
                errors,
                "gh auth git-credential: {:?} operation not supported",
                operation.to_string_lossy()
            );
            1
        }
    })
}

fn get(path: &Path, input: &mut dyn BufRead, output: &mut dyn Write) -> i32 {
    let wants = read_request(input);
    let protocol = wants.protocol.as_deref().unwrap_or("");
    let host = wants.host.as_deref().unwrap_or("");
    if protocol != "https" || host.is_empty() {
        return 1;
    }
    let Ok(credentials) = std::fs::read_to_string(path) else {
        return 1;
    };
    // An exact match on the host as git sends it: no `gist.` retry, no case folding, no port
    // dropped. Anything else gets no answer, and git asks its next source.
    let Some(token) = credential_for(&credentials, host) else {
        return 1;
    };
    let written = write!(
        output,
        "protocol=https\nhost={host}\nusername={TOKEN_USER}\npassword={token}\n"
    );
    if written.is_ok() {
        0
    } else {
        1
    }
}

#[derive(Debug, Default)]
struct Request {
    protocol: Option<String>,
    host: Option<String>,
}

/// `key=value` lines until a blank line or EOF, as gh's helper reads them
/// (`gh/pkg/cmd/auth/gitcredential/helper.go:77-102`). A `url` line sets protocol and host;
/// other keys this helper doesn't use, such as `capability[]`, `wwwauth[]` and `path`, are
/// ignored.
fn read_request(input: &mut dyn BufRead) -> Request {
    let mut request = Request::default();
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = line.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            break;
        }
        let Some((key, value)) = text.split_once('=') else {
            continue;
        };
        match key {
            "protocol" => request.protocol = Some(value.to_string()),
            "host" => request.host = Some(value.to_string()),
            "url" => {
                if let Some((scheme, rest)) = value.split_once("://") {
                    request.protocol = Some(scheme.to_string());
                    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
                    let host = authority
                        .rsplit_once('@')
                        .map_or(authority, |(_, host)| host);
                    request.host = Some(host.to_string());
                }
            }
            _ => {}
        }
    }
    request
}

/// `GIT_CONFIG_PARAMETERS` for a launched gh: the inherited value, then the launcher's two
/// entries, which clear every helper configured before them and name the run directory's script.
/// The separating space comes only after a non-empty value: git rejects leading whitespace. The
/// older `'key=value'` quoting parses in every git version.
pub fn git_config_parameters(inherited: Option<&OsStr>, run_dir: &Path) -> OsString {
    let mut value = OsString::new();
    if let Some(inherited) = inherited.filter(|inherited| !inherited.is_empty()) {
        value.push(inherited);
        value.push(" ");
    }
    value.push("'credential.helper=' 'credential.helper=!");
    value.push(run_dir.join("gh").as_os_str());
    value.push(" auth git-credential'");
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_answer(output: &[u8]) -> Option<String> {
        output
            .split(|byte| *byte == b'\n')
            .find_map(|line| line.strip_prefix(b"password="))
            .map(|password| String::from_utf8_lossy(password).into_owned())
    }

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    fn ask(credentials: &Path, operation: &str, request: &str) -> (Option<i32>, String, String) {
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let code = answer(
            Some(credentials.as_os_str()),
            &args(&["auth", "git-credential", operation]),
            &mut request.as_bytes(),
            &mut output,
            &mut errors,
        );
        (
            code,
            String::from_utf8(output).expect("utf8"),
            String::from_utf8(errors).expect("utf8"),
        )
    }

    fn credentials_file() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gfj-credentials-{}",
            crate::launcher::rundir::random_hex(8).expect("random")
        ));
        std::fs::write(&path, "forge.test real-a\nport.test:3443 real-b\n").expect("write");
        path
    }

    #[test]
    fn get_answers_only_an_exact_https_host() {
        let file = credentials_file();
        let (code, out, _) = ask(&file, "get", "protocol=https\nhost=forge.test\n\n");
        assert_eq!(code, Some(0));
        assert_eq!(
            out,
            "protocol=https\nhost=forge.test\nusername=x-access-token\npassword=real-a\n"
        );
        let (code, out, _) = ask(&file, "get", "protocol=https\nhost=port.test:3443\n");
        assert_eq!(
            (code, read_answer(out.as_bytes())),
            (Some(0), Some("real-b".to_string()))
        );
        for request in [
            "protocol=https\nhost=gist.forge.test\n",
            "protocol=https\nhost=Forge.Test\n",
            "protocol=https\nhost=port.test\n",
            "protocol=https\nhost=github.com\n",
            "protocol=http\nhost=forge.test\n",
            "host=forge.test\n",
            "protocol=https\n\nhost=forge.test\n",
        ] {
            let (code, out, _) = ask(&file, "get", request);
            assert_eq!((code, out.as_str()), (Some(1), ""), "{request:?}");
        }
        std::fs::remove_file(&file).expect("cleanup");
    }

    #[test]
    fn a_url_line_and_unused_keys_are_read_as_gh_reads_them() {
        let file = credentials_file();
        let (code, out, _) = ask(
            &file,
            "get",
            "capability[]=authtype\nwwwauth[]=Basic realm=x\nurl=https://user@forge.test/o/r.git\npath=o/r.git\n\n",
        );
        assert_eq!(code, Some(0));
        assert!(out.contains("password=real-a"), "{out}");
        std::fs::remove_file(&file).expect("cleanup");
    }

    #[test]
    fn store_and_erase_do_nothing_and_other_operations_fail() {
        let file = credentials_file();
        assert_eq!(
            ask(
                &file,
                "store",
                "protocol=https\nhost=forge.test\npassword=x\n"
            )
            .0,
            Some(0)
        );
        assert_eq!(ask(&file, "erase", "").0, Some(0));
        let (code, out, errors) = ask(&file, "approve", "");
        assert_eq!((code, out.as_str()), (Some(1), ""));
        assert!(errors.contains("not supported"), "{errors}");
        std::fs::remove_file(&file).expect("cleanup");
    }

    #[test]
    fn only_a_credential_request_with_the_variable_is_answered() {
        let request = &mut "".as_bytes();
        let mut sink = Vec::new();
        let mut errors = Vec::new();
        assert_eq!(
            answer(
                None,
                &args(&["auth", "git-credential", "get"]),
                request,
                &mut sink,
                &mut errors
            ),
            None
        );
        let path = OsString::from("/nonexistent");
        for other in [
            &["pr", "view", "1"][..],
            &["auth", "git-credential"][..],
            &["auth", "status", "get"][..],
        ] {
            assert_eq!(
                answer(Some(&path), &args(other), request, &mut sink, &mut errors),
                None,
                "{other:?}"
            );
        }
    }

    #[test]
    fn git_config_parameters_never_starts_with_a_space() {
        let run_dir = Path::new("/t/gfj-0123456789abcdef");
        let ours = "'credential.helper=' 'credential.helper=!/t/gfj-0123456789abcdef/gh auth git-credential'";
        for inherited in [None, Some(OsStr::new(""))] {
            assert_eq!(
                git_config_parameters(inherited, run_dir),
                OsString::from(ours)
            );
        }
        assert_eq!(
            git_config_parameters(Some(OsStr::new("'x.y'='z'")), run_dir),
            OsString::from(format!("'x.y'='z' {ours}"))
        );
    }
}
