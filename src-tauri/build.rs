fn git_output(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|output| !output.is_empty())
}

fn watch_git_path(name: &str) {
    if let Some(path) = git_output(&["rev-parse", "--git-path", name]) {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn main() {
    // Bake the commit into the binary for Settings → About, so it's always
    // possible to tell WHICH build a window belongs to (dev and the
    // installed app share a data dir and look identical).
    let sha = git_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ALCHEMY_GIT_SHA={sha}");
    // HEAD itself usually only names a branch: a commit on that branch moves
    // its loose ref, while packing refs moves the value into packed-refs.
    // Ask Git for physical paths because linked worktrees have their own HEAD
    // but share branch refs and packed-refs with the repository's common dir.
    watch_git_path("HEAD");
    watch_git_path("packed-refs");
    if let Some(reference) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        watch_git_path(&reference);
    }
    tauri_build::build()
}
