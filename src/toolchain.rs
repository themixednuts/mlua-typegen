use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use thiserror::Error;

use crate::toolchain_spec::required_components;

pub struct RustcRuntime {
    toolchain: &'static str,
    library_dir: Option<PathBuf>,
}

impl RustcRuntime {
    pub fn prepare(toolchain: &'static str, driver: &Path) -> Result<Self, Error> {
        ensure_installed(toolchain)?;
        let library_dir = find_library_dir(toolchain)?;
        if let Some(directory) = &library_dir {
            stage_driver_libraries(driver, directory)?;
        }
        Ok(Self {
            toolchain,
            library_dir,
        })
    }

    pub fn cargo_command(&self) -> Result<Command, Error> {
        let mut command = Command::new("rustup");
        command.args(["run", self.toolchain, "cargo"]);
        if let Some(directory) = &self.library_dir {
            prepend_library_path(&mut command, directory)?;
        }
        Ok(command)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("rustup is required; install Rust from https://rustup.rs and retry")]
    RustupMissing,
    #[error("failed to query installed rustup toolchains")]
    QueryToolchains(#[source] std::io::Error),
    #[error("rustup could not list installed toolchains: {stderr}")]
    ToolchainQueryFailed { stderr: String },
    #[error("rustup could not install required toolchain `{toolchain}` ({status})")]
    ToolchainInstallFailed {
        toolchain: &'static str,
        status: ExitStatus,
    },
    #[error("failed to query components for `{toolchain}`")]
    QueryComponents {
        toolchain: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("rustup could not list components for `{toolchain}`: {stderr}")]
    ComponentQueryFailed {
        toolchain: &'static str,
        stderr: String,
    },
    #[error("rustup could not install {components} for `{toolchain}` ({status})")]
    ComponentInstallFailed {
        toolchain: &'static str,
        components: String,
        status: ExitStatus,
    },
    #[error("failed to query the `{toolchain}` sysroot")]
    QuerySysroot {
        toolchain: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("the `{toolchain}` sysroot query failed with {status}: {stderr}")]
    SysrootQueryFailed {
        toolchain: &'static str,
        status: ExitStatus,
        stderr: String,
    },
    #[error("failed to inspect driver runtime directory `{}`", path.display())]
    ReadRuntime {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to copy driver runtime `{}` to `{}`", source_path.display(), destination.display())]
    CopyRuntime {
        source_path: PathBuf,
        destination: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to construct the compiler library search path")]
    LibraryPath(#[source] env::JoinPathsError),
}

fn ensure_installed(toolchain: &'static str) -> Result<(), Error> {
    let output = Command::new("rustup")
        .args(["toolchain", "list"])
        .output()
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                Error::RustupMissing
            } else {
                Error::QueryToolchains(source)
            }
        })?;
    if !output.status.success() {
        return Err(Error::ToolchainQueryFailed {
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let installed = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .any(|installed| installed == toolchain);
    if !installed {
        eprintln!("mlua-typegen: installing private compiler toolchain `{toolchain}`");
        let mut command = Command::new("rustup");
        command.args(["toolchain", "install", toolchain, "--profile", "minimal"]);
        for component in required_components() {
            command.args(["--component", component]);
        }
        let status = command.status().map_err(Error::QueryToolchains)?;
        if !status.success() {
            return Err(Error::ToolchainInstallFailed { toolchain, status });
        }
        return Ok(());
    }

    ensure_components(toolchain)
}

fn ensure_components(toolchain: &'static str) -> Result<(), Error> {
    let output = Command::new("rustup")
        .args(["component", "list", "--toolchain", toolchain, "--installed"])
        .output()
        .map_err(|source| Error::QueryComponents { toolchain, source })?;
    if !output.status.success() {
        return Err(Error::ComponentQueryFailed {
            toolchain,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let installed = String::from_utf8_lossy(&output.stdout);
    let missing = required_components()
        .iter()
        .copied()
        .filter(|component| {
            !installed
                .lines()
                .any(|line| line == *component || line.starts_with(&format!("{component}-")))
        })
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }

    eprintln!(
        "mlua-typegen: installing missing compiler component{} {}",
        if missing.len() == 1 { "" } else { "s" },
        missing.join(", ")
    );
    let status = Command::new("rustup")
        .args(["component", "add"])
        .args(&missing)
        .args(["--toolchain", toolchain])
        .status()
        .map_err(|source| Error::QueryComponents { toolchain, source })?;
    if !status.success() {
        return Err(Error::ComponentInstallFailed {
            toolchain,
            components: missing.join(", "),
            status,
        });
    }
    Ok(())
}

fn find_library_dir(toolchain: &'static str) -> Result<Option<PathBuf>, Error> {
    let output = Command::new("rustup")
        .args(["run", toolchain, "rustc", "--print", "sysroot"])
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::QuerySysroot { toolchain, source })?;
    if !output.status.success() {
        return Err(Error::SysrootQueryFailed {
            toolchain,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let sysroot = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let directory = if cfg!(windows) {
        sysroot.join("bin")
    } else {
        sysroot.join("lib")
    };
    Ok(directory.is_dir().then_some(directory))
}

fn prepend_library_path(command: &mut Command, directory: &Path) -> Result<(), Error> {
    let variable = if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut paths = vec![directory.to_owned()];
    if let Some(current) = env::var_os(variable) {
        paths.extend(env::split_paths(&current));
    }
    command.env(
        variable,
        env::join_paths(paths).map_err(Error::LibraryPath)?,
    );
    Ok(())
}

fn stage_driver_libraries(driver: &Path, sysroot_lib: &Path) -> Result<(), Error> {
    if !cfg!(windows) || !driver.is_file() {
        return Ok(());
    }
    let Some(driver_dir) = driver.parent() else {
        return Ok(());
    };
    let entries = fs::read_dir(sysroot_lib).map_err(|source| Error::ReadRuntime {
        path: sysroot_lib.to_owned(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| Error::ReadRuntime {
            path: sysroot_lib.to_owned(),
            source,
        })?;
        let source_path = entry.path();
        if source_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("dll")
        {
            continue;
        }
        let destination = driver_dir.join(entry.file_name());
        let needs_copy = match (fs::metadata(&source_path), fs::metadata(&destination)) {
            (Ok(source), Ok(existing)) => source.len() != existing.len(),
            (Ok(_), Err(error)) if error.kind() == std::io::ErrorKind::NotFound => true,
            (Ok(_), Err(error)) => {
                return Err(Error::ReadRuntime {
                    path: destination,
                    source: error,
                });
            }
            (Err(error), _) => {
                return Err(Error::ReadRuntime {
                    path: source_path,
                    source: error,
                });
            }
        };
        if needs_copy {
            fs::copy(&source_path, &destination).map_err(|source| Error::CopyRuntime {
                source_path,
                destination,
                source,
            })?;
        }
    }
    Ok(())
}
