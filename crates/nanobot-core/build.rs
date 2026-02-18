use std::process::Command;

fn main() {
    // Embed git short hash at compile time
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={}", hash);

    // Embed build number (from CI or commit count)
    let build_number = std::env::var("BUILD_NUMBER")
        .or_else(|_| {
            // Fallback: use git commit count as build number
            Command::new("git")
                .args(["rev-list", "--count", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .ok_or_else(|| std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "0".to_string());

    println!("cargo:rustc-env=BUILD_NUMBER={}", build_number);

    // Embed GitHub repo URL
    let repo_url = std::env::var("GITHUB_REPOSITORY")
        .map(|repo| format!("https://github.com/{}", repo))
        .unwrap_or_else(|_| "https://github.com/yukihamada/nanobot".to_string());

    println!("cargo:rustc-env=REPO_URL={}", repo_url);

    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads/");
    println!("cargo:rerun-if-env-changed=BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=GITHUB_REPOSITORY");
}
