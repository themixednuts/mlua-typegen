use std::path::{Path, PathBuf};
use std::process::Command;

const MIN_SUPPORTED_RUSTC_MINOR: u32 = 98;
const MIN_SUPPORTED_NIGHTLY: &str = "nightly-2026-06-30";
const TESTED_CURRENT_NIGHTLY: &str = "nightly-2026-06-30";

fn main() {
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-env-changed=RUSTUP_TOOLCHAIN");

    let rustc = rustc_info();
    ensure_nightly(&rustc);
    ensure_supported_nightly(&rustc);
    ensure_rustc_components(&rustc);
    stage_windows_rustc_runtime(&rustc);
    emit_rustc_api_cfgs(&rustc);

    println!(
        "cargo:rustc-env=RUSTC_VERSION={}.{}",
        rustc.major, rustc.minor
    );
    println!(
        "cargo:rustc-env=MLUA_TYPEGEN_RUSTUP_TOOLCHAIN={}",
        rustc.toolchain
    );
    println!(
        "cargo:warning=mlua-typegen: detected rustc {}.{} ({})",
        rustc.major, rustc.minor, rustc.toolchain
    );
}

struct RustcInfo {
    major: u32,
    minor: u32,
    release: String,
    commit_date: Option<String>,
    host: String,
    sysroot: PathBuf,
    toolchain: String,
}

fn rustc_info() -> RustcInfo {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = run_rustc(&rustc, &["-Vv"], "failed to run rustc -Vv");
    let sysroot = run_rustc(
        &rustc,
        &["--print", "sysroot"],
        "failed to run rustc --print sysroot",
    );

    let release = parse_version_line(&version, "release")
        .or_else(|| {
            version
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .map(str::to_string)
        })
        .unwrap_or_else(|| panic!("unexpected rustc -Vv output:\n{version}"));
    let host = parse_version_line(&version, "host")
        .unwrap_or_else(|| panic!("rustc -Vv did not include a host:\n{version}"));
    let commit_date = parse_version_line(&version, "commit-date");

    let version_core = release
        .split_once('-')
        .map_or(release.as_str(), |(core, _)| core);
    let mut parts = version_core.split('.');
    let major = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("could not parse rustc major version from {release}"));
    let minor = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("could not parse rustc minor version from {release}"));

    let sysroot = PathBuf::from(sysroot.trim());
    let toolchain = sysroot
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.contains("nightly"))
        .map(str::to_string)
        .or_else(|| std::env::var("RUSTUP_TOOLCHAIN").ok())
        .unwrap_or_else(|| "nightly".to_string());

    RustcInfo {
        major,
        minor,
        release,
        commit_date,
        host,
        sysroot,
        toolchain,
    }
}

fn run_rustc(rustc: &str, args: &[&str], error: &str) -> String {
    let output = Command::new(rustc)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{error}: {e}"));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("{error}: {stderr}");
    }
    String::from_utf8(output.stdout).unwrap_or_else(|e| panic!("rustc output was not UTF-8: {e}"))
}

fn parse_version_line(output: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}: ");
    output
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_string))
}

fn ensure_nightly(rustc: &RustcInfo) {
    if rustc.release.contains("nightly") {
        return;
    }

    panic!(
        "mlua-typegen uses rustc_private and must be built with nightly Rust. \
         Re-run the install with `cargo +nightly install ...`."
    );
}

fn ensure_supported_nightly(rustc: &RustcInfo) {
    if rustc.major == 1 && rustc.minor >= MIN_SUPPORTED_RUSTC_MINOR {
        if rustc.minor > 98 {
            println!(
                "cargo:warning=mlua-typegen: rustc {}.{} is newer than the latest tested nightly ({TESTED_CURRENT_NIGHTLY}); continuing with best-effort rustc_private compatibility",
                rustc.major, rustc.minor
            );
        }
        return;
    }

    panic!(
        "mlua-typegen supports nightly rustc >= 1.{MIN_SUPPORTED_RUSTC_MINOR} \
         (tested from {MIN_SUPPORTED_NIGHTLY} through {TESTED_CURRENT_NIGHTLY}); \
         this toolchain is {}.",
        rustc.release
    );
}

fn ensure_rustc_components(rustc: &RustcInfo) {
    let mut missing = Vec::new();
    if !has_rustc_dev(rustc) {
        missing.push("rustc-dev");
    }
    if cfg!(unix) && !has_rustc_llvm_runtime(rustc) {
        missing.push("llvm-tools-preview");
    }

    if missing.is_empty() {
        return;
    }

    println!(
        "cargo:warning=mlua-typegen: missing {} for {}; installing with rustup",
        missing.join(", "),
        rustc.toolchain,
    );

    let status = Command::new("rustup")
        .args(["component", "add"])
        .args(&missing)
        .args(["--toolchain", &rustc.toolchain])
        .status();

    if matches!(status, Ok(status) if status.success())
        && has_rustc_dev(rustc)
        && (!cfg!(unix) || has_rustc_llvm_runtime(rustc))
    {
        return;
    }

    panic!(
        "mlua-typegen needs rustc-dev and, on Unix, llvm-tools-preview for toolchain `{}`. \
         Automatic installation failed; run `rustup component add rustc-dev \
         llvm-tools-preview --toolchain {}`.",
        rustc.toolchain, rustc.toolchain
    );
}

fn emit_rustc_api_cfgs(rustc: &RustcInfo) {
    println!("cargo:rustc-check-cfg=cfg(rustc_alias_kind_field)");
    println!("cargo:rustc-check-cfg=cfg(rustc_instantiate_unnormalized)");
    println!("cargo:rustc-check-cfg=cfg(rustc_driver_catch_returns_exit_code)");
    println!("cargo:rustc-check-cfg=cfg(rustc_filename_prefer_local)");
    println!("cargo:rustc-check-cfg=cfg(rustc_alias_is_rigid)");
    println!("cargo:rustc-check-cfg=cfg(rustc_alias_kind_struct_variants)");
    println!("cargo:rustc-check-cfg=cfg(rustc_early_binder_bind_tcx)");

    if rustc.minor >= 97 {
        println!("cargo:rustc-cfg=rustc_alias_kind_field");
        println!("cargo:rustc-cfg=rustc_instantiate_unnormalized");
    }

    if rustc_private_api_at_least(rustc, 98, "2026-06-29") {
        println!("cargo:rustc-cfg=rustc_alias_is_rigid");
        println!("cargo:rustc-cfg=rustc_alias_kind_struct_variants");
        println!("cargo:rustc-cfg=rustc_early_binder_bind_tcx");
    }

    if rustc.minor >= 95 {
        println!("cargo:rustc-cfg=rustc_driver_catch_returns_exit_code");
    }

    if rustc.minor >= 94 {
        println!("cargo:rustc-cfg=rustc_filename_prefer_local");
    }
}

fn rustc_private_api_at_least(rustc: &RustcInfo, minor: u32, commit_date: &str) -> bool {
    if rustc.major > 1 || rustc.minor > minor {
        return true;
    }
    if rustc.major < 1 || rustc.minor < minor {
        return false;
    }

    rustc
        .commit_date
        .as_deref()
        .is_some_and(|date| date >= commit_date)
}

fn stage_windows_rustc_runtime(rustc: &RustcInfo) {
    if !cfg!(windows) {
        return;
    }

    let Some(profile_dir) = cargo_profile_dir() else {
        println!(
            "cargo:warning=mlua-typegen: could not locate Cargo profile directory for rustc DLL staging"
        );
        return;
    };

    let sysroot_bin = rustc.sysroot.join("bin");
    let Ok(entries) = std::fs::read_dir(&sysroot_bin) else {
        println!(
            "cargo:warning=mlua-typegen: could not read rustc DLL directory {}",
            sysroot_bin.display()
        );
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("dll") {
            continue;
        }

        let dest = profile_dir.join(entry.file_name());
        let should_copy = match (std::fs::metadata(&path), std::fs::metadata(&dest)) {
            (Ok(src), Ok(dst)) => src.len() != dst.len(),
            (Ok(_), Err(_)) => true,
            _ => false,
        };

        if should_copy && let Err(error) = std::fs::copy(&path, &dest) {
            println!(
                "cargo:warning=mlua-typegen: failed to stage {} into {}: {error}",
                path.display(),
                dest.display()
            );
        }
    }
}

fn cargo_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR")?);
    out_dir.parent()?.parent()?.parent().map(Path::to_path_buf)
}

fn has_rustc_dev(rustc: &RustcInfo) -> bool {
    let lib_dir = rustc_dev_lib_dir(&rustc.sysroot, &rustc.host);
    let Ok(entries) = std::fs::read_dir(lib_dir) else {
        return false;
    };

    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.contains("rustc_driver"))
    })
}

fn has_rustc_llvm_runtime(rustc: &RustcInfo) -> bool {
    let lib_dir = rustc_dev_lib_dir(&rustc.sysroot, &rustc.host);
    let Ok(entries) = std::fs::read_dir(lib_dir) else {
        return false;
    };

    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("libLLVM-") && name.contains("-rust-"))
    })
}

fn rustc_dev_lib_dir(sysroot: &Path, host: &str) -> PathBuf {
    sysroot.join("lib").join("rustlib").join(host).join("lib")
}
