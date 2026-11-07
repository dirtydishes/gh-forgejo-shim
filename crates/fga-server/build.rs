//! Sets `FGA_PRODUCT_VERSION` to the workspace's version, the release's, which usage reports name.
//! fga-server keeps its own crate version.
fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    println!("cargo:rerun-if-changed={path}");
    let text = std::fs::read_to_string(path).expect("the workspace's Cargo.toml");
    let version = text
        .split("[workspace.package]")
        .nth(1)
        .and_then(|section| section.split("\n[").next())
        .and_then(|section| {
            section.lines().find_map(|line| {
                let value = line
                    .trim()
                    .strip_prefix("version")?
                    .trim()
                    .strip_prefix('=')?;
                Some(value.trim().trim_matches('"').to_string())
            })
        })
        .expect("a version in [workspace.package]");
    println!("cargo:rustc-env=FGA_PRODUCT_VERSION={version}");
}
