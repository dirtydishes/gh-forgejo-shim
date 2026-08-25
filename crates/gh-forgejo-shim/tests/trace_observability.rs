#[allow(dead_code)]
mod support;

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use gh_forgejo_shim::forgejo::{ForgejoClient, RepoRef};
use serde_json::{json, Value};
use support::{CliFixture, TestResult};

#[test]
fn managed_gh_traced_watch_keeps_inherited_streaming() -> TestResult {
    let fixture = CliFixture::new()?;
    let started_path = fixture.root().join("watch-started");
    let release_path = fixture.root().join("watch-release");
    fixture.write_executable(
        "gh",
        concat!(
            "#!/bin/sh\n",
            "printf 'first\\n'\n",
            ": > \"$FJ_TEST_STARTED\"\n",
            "while [ ! -f \"$FJ_TEST_RELEASE\" ]; do /bin/sleep 0.01; done\n",
            "printf 'last\\n'\n",
        ),
    )?;

    let mut child = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_TRACE", fixture.root().join("watch-trace.jsonl"))
        .env("FJ_TEST_STARTED", &started_path)
        .env("FJ_TEST_RELEASE", &release_path)
        .arg("gh")
        .args([
            "pr",
            "checks",
            "13",
            "--watch",
            "--repo",
            "github.com/owner/repo",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("managed gh stdout was not piped"))?;
    let (first_line_sender, first_line_receiver) = mpsc::channel();
    let reader = thread::spawn(move || -> io::Result<String> {
        let mut stdout = BufReader::new(stdout);
        let mut first_line = String::new();
        stdout.read_line(&mut first_line)?;
        let _ = first_line_sender.send(first_line.clone());
        let mut remaining = String::new();
        stdout.read_to_string(&mut remaining)?;
        Ok(first_line + &remaining)
    });

    let started_deadline = Instant::now() + Duration::from_secs(1);
    while !started_path.exists() && Instant::now() < started_deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let early_line = first_line_receiver.recv_timeout(Duration::from_millis(300));
    fs::write(&release_path, "release\n")?;
    let status = child.wait()?;
    let stdout = reader
        .join()
        .map_err(|_| io::Error::other("stdout reader thread panicked"))??;

    assert!(
        started_path.exists(),
        "fake gh did not reach its wait point"
    );
    assert_eq!(early_line?, "first\n", "first line was buffered until exit");
    assert!(status.success());
    assert_eq!(stdout, "first\nlast\n");
    Ok(())
}

#[test]
fn managed_gh_traced_help_keeps_unknown_output_counts() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\nprintf 'delegated help\\n'\n")?;
    let trace_path = fixture.root().join("help-trace.jsonl");

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_TRACE", &trace_path)
        .arg("gh")
        .args([
            "pr",
            "view",
            "--repo",
            "git.example.com/owner/repo",
            "--help",
        ])
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?, "delegated help\n");
    let record: Value = serde_json::from_str(fs::read_to_string(trace_path)?.trim())?;
    assert_eq!(record["command_class"], "pr-view");
    assert_eq!(record["stdout"], json!({"state": "unknown", "bytes": null}));
    assert_eq!(record["stderr"], json!({"state": "unknown", "bytes": null}));
    Ok(())
}

#[test]
fn forgejo_client_shares_one_deadline_across_sequential_requests() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let host = listener.local_addr()?.to_string();
    let handle = thread::spawn(move || -> io::Result<()> {
        for (delay, body) in [
            (Duration::from_millis(100), r#"{"id":1}"#),
            (Duration::from_millis(250), r#"{"id":2}"#),
        ] {
            let (mut stream, _) = listener.accept()?;
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request)?;
            thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
        Ok(())
    });
    let client = ForgejoClient::new(None)
        .with_scheme("http")
        .with_timeout(Duration::from_millis(300));
    let repo = RepoRef::new(host, "owner", "repo");

    assert_eq!(client.get_repo(&repo)?["id"], 1);
    let second = client.get_repo(&repo);
    let _ = handle.join();

    let error = second
        .err()
        .ok_or_else(|| io::Error::other("second request reset the command deadline"))?;
    assert_eq!(error.to_string(), "Forgejo command deadline exceeded");
    Ok(())
}
