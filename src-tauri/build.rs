fn main() {
    // Bake the commit into the binary for Settings → About, so it's always
    // possible to tell WHICH build a window belongs to (dev and the
    // installed app share a data dir and look identical).
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ALCHEMY_GIT_SHA={sha}");
    // Re-run when HEAD moves so the sha stays honest across commits.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    tauri_build::build()
}
