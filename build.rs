/// Build script for mlua-typegen.
///
/// Emits cfg flags based on the rustc version to handle API differences
/// between nightly versions. The driver binary uses `rustc_private` crates
/// whose APIs change without notice between nightlies.
///
/// To add support for a new API change:
/// 1. Find the version boundary
/// 2. Add a version check below with a descriptive cfg name
/// 3. Use `#[cfg(flag)]` / `#[cfg(not(flag))]` in code
fn main() {
    let _version = rustc_version();

    // Add version-gated cfg flags here as needed. Example:
    // if version >= (1, 97) {
    //     println!("cargo:rustc-cfg=has_new_api");
    // }
}

fn rustc_version() -> (u32, u32) {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = std::process::Command::new(&rustc)
        .arg("--version")
        .output()
        .expect("failed to run rustc --version");
    let version_str = String::from_utf8(output.stdout).expect("invalid utf8 from rustc");

    // Parse "rustc 1.96.0-nightly (hash date)" -> (1, 96)
    let version_part = version_str
        .split_whitespace()
        .nth(1)
        .expect("unexpected rustc --version format");
    let mut parts = version_part.split('.');
    let major: u32 = parts.next().unwrap().parse().unwrap();
    let minor: u32 = parts.next().unwrap().parse().unwrap();

    println!("cargo:rustc-env=RUSTC_VERSION={major}.{minor}");
    println!("cargo:warning=mlua-typegen: detected rustc {major}.{minor}");

    (major, minor)
}
