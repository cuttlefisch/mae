fn main() {
    // Capture git hash at compile time for transcript metadata.
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    {
        if let Ok(hash) = String::from_utf8(output.stdout) {
            let hash = hash.trim();
            if !hash.is_empty() {
                println!("cargo:rustc-env=MAE_GIT_HASH={}", hash);
            }
        }
    }
    // Re-run if HEAD changes (new commits).
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}
