use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MOON_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=MOON_GIT_DIRTY");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=tests");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR");
    let git_commit = env::var("MOON_GIT_COMMIT")
        .map(|value| validated_commit(value, "MOON_GIT_COMMIT"))
        .unwrap_or_else(|_| commit_from_checkout(Path::new(&manifest_dir)));
    let git_dirty = env::var("MOON_GIT_DIRTY")
        .map(|value| validated_dirty(value, "MOON_GIT_DIRTY"))
        .unwrap_or_else(|_| dirty_from_checkout(Path::new(&manifest_dir)));
    let target = env::var("TARGET").expect("Cargo provides TARGET");
    let profile = env::var("PROFILE").expect("Cargo provides PROFILE");

    println!("cargo:rustc-env=MOON_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=MOON_GIT_DIRTY={git_dirty}");
    println!("cargo:rustc-env=MOON_BUILD_TARGET={target}");
    println!("cargo:rustc-env=MOON_BUILD_PROFILE={profile}");
}

fn dirty_from_checkout(manifest_dir: &Path) -> String {
    let output = Command::new("git")
        .args([
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--",
            ".",
        ])
        .current_dir(manifest_dir)
        .output();
    match output {
        Ok(output) if output.status.success() && output.stdout.is_empty() => "false".to_owned(),
        Ok(output) if output.status.success() => "true".to_owned(),
        _ => "unknown".to_owned(),
    }
}

fn commit_from_checkout(manifest_dir: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(manifest_dir)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            validated_commit(value, "git rev-parse")
        }
        _ => "unknown".to_owned(),
    }
}

fn validated_commit(value: String, source: &str) -> String {
    let valid_length = (40..=64).contains(&value.len());
    if valid_length && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        value.to_ascii_lowercase()
    } else {
        panic!("{source} must contain a full hexadecimal Git commit id");
    }
}

fn validated_dirty(value: String, source: &str) -> String {
    match value.as_str() {
        "true" | "false" | "unknown" => value,
        _ => panic!("{source} must be true, false, or unknown"),
    }
}
