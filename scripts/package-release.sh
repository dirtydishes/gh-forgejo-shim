#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/package-release.sh [TARGET_TRIPLE]

Build the Rust binaries and create a GitHub release archive under dist/.
The archive extracts with gh-forgejo-shim and gfj at its root so Homebrew and
manual installs can place the binaries directly.

Environment:
  CARGO_BUILD_TARGET  Target triple used when TARGET_TRIPLE is omitted.
  CARGO_TARGET_DIR    Cargo target directory. Defaults to target/.
  DIST_DIR            Output directory. Defaults to dist/.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ $# -gt 1 ]]; then
  usage >&2
  exit 2
fi

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

version="$(cargo pkgid -p gh-forgejo-shim | awk -F'[#@]' '{ print $NF }')"
target_triple="${1:-${CARGO_BUILD_TARGET:-}}"

build_args=(build --release --locked -p gh-forgejo-shim)
target_dir="${CARGO_TARGET_DIR:-target}"

if [[ -z "$target_triple" ]]; then
  target_triple="$(rustc -vV | awk '/^host:/ { print $2 }')"
  release_dir="$target_dir/release"
else
  build_args+=(--target "$target_triple")
  release_dir="$target_dir/$target_triple/release"
fi

cargo "${build_args[@]}"

bin_suffix=""
case "$target_triple" in
  *windows*) bin_suffix=".exe" ;;
esac

dist_dir="${DIST_DIR:-dist}"
archive_base="gh-forgejo-shim-${version}-${target_triple}"
archive_path="$dist_dir/${archive_base}.tar.gz"
stage_parent="$(mktemp -d)"
stage_dir="$stage_parent/$archive_base"

cleanup() {
  rm -rf "$stage_parent"
}
trap cleanup EXIT

install -d "$stage_dir/docs" "$dist_dir"
install -m 0755 "$release_dir/gh-forgejo-shim${bin_suffix}" "$stage_dir/gh-forgejo-shim${bin_suffix}"
install -m 0755 "$release_dir/gfj${bin_suffix}" "$stage_dir/gfj${bin_suffix}"
install -m 0644 README.md "$stage_dir/README.md"
install -m 0644 LICENSE "$stage_dir/LICENSE"
install -m 0644 docs/installation.md "$stage_dir/docs/installation.md"
install -m 0644 docs/pipx-to-rust-migration.md "$stage_dir/docs/pipx-to-rust-migration.md"
install -m 0644 docs/rollback.md "$stage_dir/docs/rollback.md"
printf '%s\n' "$version" > "$stage_dir/VERSION"

COPYFILE_DISABLE=1 tar -czf "$archive_path" -C "$stage_dir" .

if command -v sha256sum >/dev/null 2>&1; then
  (
    cd "$dist_dir"
    sha256sum "${archive_base}.tar.gz" > "${archive_base}.tar.gz.sha256"
    sha256sum ./*.tar.gz > checksums.txt
  )
else
  (
    cd "$dist_dir"
    shasum -a 256 "${archive_base}.tar.gz" > "${archive_base}.tar.gz.sha256"
    shasum -a 256 ./*.tar.gz > checksums.txt
  )
fi

echo "$archive_path"
