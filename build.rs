//! Exposes the target triple of the build, so `repo-sweep --version` says which archive is running
//! (the same name used by the release archives, e.g. `aarch64-apple-darwin`).

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=REPO_SWEEP_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
