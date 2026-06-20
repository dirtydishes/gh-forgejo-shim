use gh_forgejo_shim::cli::{self, BinaryName};

fn main() {
    std::process::exit(cli::run_from_env(BinaryName::Gfj));
}
