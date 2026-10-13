//! Build script to capture build-time information

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Build timestamp (Unix timestamp)
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", timestamp);
    
    // Git commit hash (short)
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
    
    // Rerun if git HEAD changes or Cargo.toml changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
