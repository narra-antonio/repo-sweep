//! `--version` must name the build target, so bug reports say where they come from.

use std::process::Command;

#[test]
fn version_includes_target() {
    let out = Command::new(env!("CARGO_BIN_EXE_repo-sweep"))
        .arg("--version")
        .output()
        .expect("run repo-sweep --version");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf-8");
    let expected = format!(
        "repo-sweep {} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("REPO_SWEEP_TARGET")
    );
    assert_eq!(text.trim(), expected);
}
