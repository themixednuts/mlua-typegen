use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut output_dir = PathBuf::from("lua-types");
    let mut emmylua = false;
    let mut cargo_args = Vec::new();

    // Skip "mlua-typegen" if invoked as `cargo mlua-typegen`
    let args: Vec<String> = env::args().skip(1).collect();
    let mut args_iter = args.iter().peekable();

    // Skip the subcommand name if cargo passes it
    if args_iter.peek().is_some_and(|a| a.as_str() == "mlua-typegen") {
        args_iter.next();
    }

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
                emmylua = true;
            }
            _ => {
                cargo_args.push(arg.clone());
            }
        }
    }

    // Find the driver binary (installed alongside this binary)
    let driver = find_driver();

    // Run cargo check with our driver as RUSTC_WRAPPER
    let mut cmd = Command::new("cargo");
    cmd.arg("check")
        .args(&cargo_args)
        .env("RUSTC_WRAPPER", &driver)
        .env("MLUA_TYPEGEN_OUTPUT", output_dir.to_string_lossy().as_ref());

    // Ensure the toolchain's shared libraries (rustc_driver, std) are findable.
    // The driver links against these DLLs and needs them on the library search path.
    if let Some(sysroot_lib) = find_sysroot_lib_dir() {
        prepend_lib_path(&mut cmd, &sysroot_lib);
    }

    if emmylua {
        cmd.env("MLUA_TYPEGEN_EMMYLUA", "1");
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
    // The driver binary should be next to this binary
    let self_path = env::current_exe().expect("failed to get current exe path");
    let dir = self_path.parent().expect("exe has no parent dir");

    let driver = dir.join("mlua-typegen-driver");
    if driver.exists() {
        return driver;
    }

    // Try with .exe extension on Windows
    let driver_exe = dir.join("mlua-typegen-driver.exe");
    if driver_exe.exists() {
        return driver_exe;
    }

    // Fall back to PATH
    PathBuf::from("mlua-typegen-driver")
}

/// Find the directory containing rustc_driver shared libraries.
/// This is typically `<sysroot>/bin` on Windows or `<sysroot>/lib` on Unix.
fn find_sysroot_lib_dir() -> Option<PathBuf> {
    // Ask rustc for its sysroot
    let output = Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
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
