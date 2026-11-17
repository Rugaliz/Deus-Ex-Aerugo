//! The build's place in the history for the main menu's version line: `AERUGO_BUILD` when set (the release container
//! has no git checkout: `scripts/release.sh` passes it in, worked out the same way), else the tag on the commit built
//! (`v0.4.2`), or the short commit (`56fba9b`), with `+` when files had uncommitted changes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

fn main() {
    println!("cargo:rerun-if-env-changed=AERUGO_BUILD");
    let build = std::env::var("AERUGO_BUILD").ok().filter(|s| !s.is_empty()).or_else(|| {
        let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").ok()?);
        // described again when HEAD moves, a branch or tag changes, or the index does (for `-dirty`)
        for (flag, files) in [("--git-dir", &["HEAD", "index"][..]), ("--git-common-dir", &["packed-refs", "refs/heads", "refs/tags"][..])] {
            if let Some(d) = git(&dir, &["rev-parse", flag]) {
                let d = dir.join(d);
                for f in files {
                    println!("cargo:rerun-if-changed={}", d.join(f).display());
                }
            }
        }
        if let Some(r) = git(&dir, &["symbolic-ref", "-q", "HEAD"]).zip(git(&dir, &["rev-parse", "--git-common-dir"])) {
            println!("cargo:rerun-if-changed={}", dir.join(r.1).join(r.0).display());
        }
        let id = git(&dir, &["describe", "--tags", "--exact-match"]).or_else(|| git(&dir, &["rev-parse", "--short=7", "HEAD"]))?;
        let dirty = git(&dir, &["status", "--porcelain", "--untracked-files=no"]).is_some();
        Some(if dirty { format!("{id}+") } else { id })
    });
    println!("cargo:rustc-env=AERUGO_BUILD={}", build.unwrap_or_default());
}
