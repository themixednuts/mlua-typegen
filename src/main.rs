use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::SystemTime;

fn main() -> ExitCode {
    let mut output_dir = PathBuf::from("lua-types");
    let mut emit_emmylua = false;
    let mut cargo_args = Vec::new();

    let args: Vec<String> = env::args().skip(1).collect();
    let mut args_iter = args.iter().peekable();

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--output" | "-o" => {
                if let Some(dir) = args_iter.next() {
                    output_dir = PathBuf::from(dir);
                } else {
                    eprintln!("error: --output requires a directory argument");
                    return ExitCode::FAILURE;
                }
            }
            "--emmylua" => {
                emit_emmylua = true;
            }
            _ => {
                cargo_args.push(arg.clone());
            }
        }
    }

    if output_dir.is_relative()
        && let Ok(cwd) = env::current_dir()
    {
        output_dir = cwd.join(output_dir);
    }

    // If the output directory is missing, invalidate cargo's freshness check so
    // the driver re-runs. Without this, cargo replays cached diagnostics but
    // doesn't re-invoke the workspace wrapper, so stubs never get regenerated.
    if !output_dir.exists() {
        invalidate_source_mtime();
    }

    // Find the driver binary (installed alongside this binary)
    let driver = find_driver();

    // Run cargo check with our driver as the workspace wrapper.
    // RUSTC_WORKSPACE_WRAPPER only wraps workspace crates (not deps) and gets
    // its own artifact cache, so a prior `cargo check` won't suppress our analysis.
    // The driver links against nightly rustc internals, so we force the nightly
    // toolchain via RUSTUP_TOOLCHAIN (unless the user already set it).
    let mut cmd = Command::new("cargo");
    cmd.arg("check")
        .args(&cargo_args)
        .env("RUSTC_WORKSPACE_WRAPPER", &driver)
        .env("MLUA_TYPEGEN_OUTPUT", output_dir.to_string_lossy().as_ref());

    if env::var("RUSTUP_TOOLCHAIN").is_err() {
        cmd.env("RUSTUP_TOOLCHAIN", "nightly");
    }

    // Ensure the toolchain's shared libraries (rustc_driver, std) are findable.
    // The driver links against these DLLs and needs them on the library search path.
    if let Some(sysroot_lib) = find_sysroot_lib_dir() {
        stage_driver_runtime(&driver, &sysroot_lib);
        prepend_lib_path(&mut cmd, &sysroot_lib);
    }

    if emit_emmylua {
        cmd.env("MLUA_TYPEGEN_EMMYLUA", "1");
    }

    if let Ok(value) = env::var("MLUA_TYPEGEN_TRACE") {
        cmd.env("MLUA_TYPEGEN_TRACE", value);
    }
    if let Ok(value) = env::var("MLUA_TYPEGEN_TRACE_FILE") {
        cmd.env("MLUA_TYPEGEN_TRACE_FILE", value);
    }
    let status = cmd.status();

    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => ExitCode::from(s.code().unwrap_or(1) as u8),
        Err(e) => {
            eprintln!("error: failed to run cargo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn find_driver() -> PathBuf {
    let self_path = env::current_exe().expect("failed to get current exe path");
    let dir = self_path.parent().expect("exe has no parent dir");

    let mut candidate_dirs = vec![dir.to_path_buf()];
    if dir.file_name().and_then(|s| s.to_str()) == Some("deps")
        && let Some(parent) = dir.parent()
    {
        candidate_dirs.push(parent.to_path_buf());
    }

    for dir in candidate_dirs {
        let driver = dir.join("mlua-typegen-driver");
        if driver.exists() {
            return driver;
        }

        let driver_exe = dir.join("mlua-typegen-driver.exe");
        if driver_exe.exists() {
            return driver_exe;
        }
    }

    PathBuf::from("mlua-typegen-driver")
}

/// Find the directory containing rustc_driver shared libraries.
/// This is typically `<sysroot>/bin` on Windows or `<sysroot>/lib` on Unix.
fn find_sysroot_lib_dir() -> Option<PathBuf> {
    // Ask nightly rustc for its sysroot (the driver links against nightly libs)
    let output = Command::new("rustup")
        .args(["run", "nightly", "rustc", "--print", "sysroot"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let sysroot = String::from_utf8(output.stdout).ok()?;
    let sysroot = PathBuf::from(sysroot.trim());

    // On Windows, DLLs are in <sysroot>/bin
    let bin_dir = sysroot.join("bin");
    if bin_dir.exists() {
        return Some(bin_dir);
    }

    // On Unix, shared objects are in <sysroot>/lib
    let lib_dir = sysroot.join("lib");
    if lib_dir.exists() {
        return Some(lib_dir);
    }

    None
}

/// Touch a source file's mtime to force cargo to re-check the workspace crate.
/// This is metadata-only — file content is unchanged, so git status stays clean.
fn invalidate_source_mtime() {
    // Try common entry points in order of likelihood
    for name in ["src/lib.rs", "src/main.rs"] {
        let path = Path::new(name);
        if path.exists()
            && let Ok(file) = fs::File::options().write(true).open(path)
        {
            let _ = file.set_times(fs::FileTimes::new().set_modified(SystemTime::now()));
            return;
        }
    }
}

/// Prepend a directory to the platform's library search path.
fn prepend_lib_path(cmd: &mut Command, dir: &Path) {
    // On Windows, DLLs are found via PATH
    // On Unix, shared libraries are found via LD_LIBRARY_PATH (Linux) or DYLD_LIBRARY_PATH (macOS)
    if cfg!(windows) {
        let current = env::var("PATH").unwrap_or_default();
        let new_path = format!("{};{}", dir.display(), current);
        cmd.env("PATH", new_path);
    } else if cfg!(target_os = "macos") {
        let current = env::var("DYLD_LIBRARY_PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.display(), current);
        cmd.env("DYLD_LIBRARY_PATH", new_path);
    } else {
        let current = env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.display(), current);
        cmd.env("LD_LIBRARY_PATH", new_path);
    }
}

fn stage_driver_runtime(driver: &Path, sysroot_lib: &Path) {
    if !cfg!(windows) {
        return;
    }

    let Some(driver_dir) = driver.parent() else {
        return;
    };

    let Ok(entries) = fs::read_dir(sysroot_lib) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("dll") {
            continue;
        }

        let dest = driver_dir.join(entry.file_name());
        let should_copy = match (fs::metadata(&path), fs::metadata(&dest)) {
            (Ok(src), Ok(dst)) => src.len() != dst.len(),
            (Ok(_), Err(_)) => true,
            _ => false,
        };

        if should_copy {
            let _ = fs::copy(&path, &dest);
        }
    }
}
