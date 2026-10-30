//! The Python driver uses only the standard library, also used by oracle capture and seeding.
use std::path::Path;
use std::process::Command;

#[test]
fn contract_driver_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = Command::new("python3")
        .args([
            "-m",
            "unittest",
            "discover",
            "-s",
            "scripts/contract",
            "-p",
            "test_*.py",
        ])
        .current_dir(root)
        .status()
        .expect("python3 is required for contract driver tests");
    assert!(status.success(), "contract driver tests failed");
}

#[test]
#[ignore = "requires seeded Forgejo 15; use scripts/contract/run.sh"]
fn matches_real_gh() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = Command::new("python3")
        .arg("scripts/contract/runner.py")
        .env("GFJS_SHIM_BINARY", env!("CARGO_BIN_EXE_gh-forgejo-shim"))
        .current_dir(root)
        .status()
        .expect("python3 is required for the contract runner");
    assert!(status.success(), "contract comparison failed");
}
