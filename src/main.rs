use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

use mlua_typegen::driver_protocol::INVOCATION_LEDGER_ENV;
use mlua_typegen::generation::{
    AssembleError, LockError, OutputLock, PublishError, PublishedSnapshot, ReachabilityError,
    RunWorkspace, WorkspaceError, assemble_reports, resolve_reachable_reports,
};
use mlua_typegen::{
    Dialect,
    config::{self, ParsedConfig, ResolvedConfig},
};
use thiserror::Error;

const BUILD_RUSTUP_TOOLCHAIN: &str = env!("MLUA_TYPEGEN_RUSTUP_TOOLCHAIN");

fn main() -> ExitCode {
    match run() {
        Ok(published) => {
            eprintln!(
                "mlua-typegen: atomically published {} compiler units to {}",
                published.unit_count,
                published.path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            if let Some(snapshot) = error.preserved_snapshot() {
                eprintln!(
                    "mlua-typegen: incomplete run; preserved previous snapshot at {}",
                    snapshot.display()
                );
            } else if error.is_incomplete_run() {
                eprintln!("mlua-typegen: incomplete run; no snapshot was published");
            }
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<PublishedSnapshot, AppError> {
    let options = CliOptions::parse(env::args_os().skip(1))?;
    let output_dir = absolute_path(&options.output_dir)?;
    let output_lock = OutputLock::acquire(&output_dir)?;
    let workspace = RunWorkspace::create()?;
    let parsed_config = options
        .config_path
        .as_ref()
        .map(|path| read_parsed_config(path))
        .transpose()?;
    let metadata = cargo_metadata(&options.cargo_args)?;
    let resolved_config = match parsed_config {
        Some(config) => config.resolve(&metadata)?,
        None => {
            let default_path = metadata.workspace_root.join(config::DEFAULT_CONFIG_FILE);
            if default_path.is_file() {
                read_parsed_config(default_path.as_std_path())?.resolve(&metadata)?
            } else {
                ResolvedConfig::empty()
            }
        }
    };
    let config_snapshot = workspace.snapshot_config(&resolved_config.snapshot_bytes()?)?;
    let driver = find_driver()?;

    let mut command = Command::new("rustup");
    command
        .arg("run")
        .arg(BUILD_RUSTUP_TOOLCHAIN)
        .arg("cargo")
        .arg("check")
        .args(&options.cargo_args)
        .env("RUSTC_WRAPPER", &driver)
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env("MLUA_TYPEGEN_OUTPUT", workspace.report_dir())
        .env(INVOCATION_LEDGER_ENV, workspace.invocation_ledger())
        .env(config::CONFIG_SNAPSHOT_ENV, config_snapshot)
        .env("CARGO_TARGET_DIR", workspace.cargo_target_dir())
        .env("RUSTUP_TOOLCHAIN", BUILD_RUSTUP_TOOLCHAIN);

    if let Some(sysroot_lib) = find_sysroot_lib_dir(BUILD_RUSTUP_TOOLCHAIN)? {
        stage_driver_runtime(&driver, &sysroot_lib)?;
        prepend_lib_path(&mut command, &sysroot_lib)?;
    }
    if options.emit_emmylua {
        command.env("MLUA_TYPEGEN_EMMYLUA", "1");
    } else {
        command.env_remove("MLUA_TYPEGEN_EMMYLUA");
    }
    pass_through_env(&mut command, "MLUA_TYPEGEN_TRACE");
    pass_through_env(&mut command, "MLUA_TYPEGEN_TRACE_FILE");

    let status = command.status().map_err(AppError::RunCargo)?;
    if !status.success() {
        let preserved_snapshot = output_lock
            .snapshot_path()
            .is_file()
            .then(|| output_lock.snapshot_path());
        return Err(AppError::CargoFailed {
            status,
            preserved_snapshot,
        });
    }

    resolve_reachable_reports(
        &workspace,
        &driver,
        config_snapshot,
        if options.emit_emmylua {
            Dialect::EmmyLua
        } else {
            Dialect::LuaLs
        },
    )?;

    let snapshot = assemble_reports(workspace.report_dir())?;
    let unused_mappings = resolved_config
        .mapping_ids()
        .filter(|id| !snapshot.used_mappings().contains(*id))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let published = output_lock.publish(&snapshot).map_err(AppError::from)?;
    for id in unused_mappings {
        eprintln!("warning: explicit mapping `{id}` was not used by any complete compiler unit");
    }
    Ok(published)
}

#[derive(Debug)]
struct CliOptions {
    output_dir: PathBuf,
    config_path: Option<PathBuf>,
    emit_emmylua: bool,
    cargo_args: Vec<OsString>,
}

impl CliOptions {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, AppError> {
        let mut output_dir = PathBuf::from("lua-types");
        let mut emit_emmylua = false;
        let mut config_path = None;
        let mut cargo_args = Vec::new();
        let mut args = args.into_iter();

        while let Some(argument) = args.next() {
            match argument.to_str() {
                Some("--output" | "-o") => {
                    output_dir = args
                        .next()
                        .map(PathBuf::from)
                        .ok_or(AppError::MissingOutput)?;
                }
                Some("--emmylua") => emit_emmylua = true,
                Some("--config") => {
                    config_path = Some(
                        args.next()
                            .map(PathBuf::from)
                            .ok_or(AppError::MissingConfig)?,
                    );
                }
                _ => cargo_args.push(argument),
            }
        }

        Ok(Self {
            output_dir,
            config_path,
            emit_emmylua,
            cargo_args,
        })
    }
}

#[derive(Debug, Error)]
enum AppError {
    #[error("--output requires a directory argument")]
    MissingOutput,
    #[error("--config requires a file argument")]
    MissingConfig,
    #[error("failed to read mapping configuration `{}`", path.display())]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("mapping configuration is invalid: {0}")]
    InvalidConfig(#[from] config::ConfigErrors),
    #[error("failed to encode the validated mapping configuration")]
    EncodeConfig(#[from] serde_json::Error),
    #[error("failed to query Cargo workspace metadata")]
    CargoMetadata(#[from] cargo_metadata::Error),
    #[error("failed to resolve the current directory")]
    CurrentDirectory(#[source] std::io::Error),
    #[error("failed to locate the mlua-typegen executable")]
    CurrentExecutable(#[source] std::io::Error),
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
    #[error("failed to run cargo through rustup")]
    RunCargo(#[source] std::io::Error),
    #[error("cargo check failed with {status}")]
    CargoFailed {
        status: ExitStatus,
        preserved_snapshot: Option<PathBuf>,
    },
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Reachability(#[from] ReachabilityError),
    #[error(transparent)]
    Assemble(#[from] AssembleError),
    #[error(transparent)]
    Publish(#[from] PublishError),
}

fn read_parsed_config(path: &Path) -> Result<ParsedConfig, AppError> {
    let source = std::fs::read_to_string(path).map_err(|source| AppError::ReadConfig {
        path: path.to_owned(),
        source,
    })?;
    ParsedConfig::parse(&source).map_err(AppError::from)
}

fn cargo_metadata(cargo_args: &[OsString]) -> Result<cargo_metadata::Metadata, AppError> {
    let mut command = cargo_metadata::MetadataCommand::new();
    if let Some(path) = manifest_path(cargo_args) {
        command.manifest_path(path);
    }
    command.exec().map_err(AppError::from)
}

fn manifest_path(cargo_args: &[OsString]) -> Option<PathBuf> {
    let mut arguments = cargo_args.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--manifest-path" {
            return arguments.next().map(PathBuf::from);
        }
        if let Some(argument) = argument.to_str()
            && let Some(path) = argument.strip_prefix("--manifest-path=")
        {
            return Some(PathBuf::from(path));
        }
    }
    None
}

impl AppError {
    fn preserved_snapshot(&self) -> Option<&Path> {
        match self {
            Self::CargoFailed {
                preserved_snapshot: Some(path),
                ..
            } => Some(path),
            _ => None,
        }
    }

    const fn is_incomplete_run(&self) -> bool {
        matches!(self, Self::CargoFailed { .. })
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, AppError> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .map_err(AppError::CurrentDirectory)
    }
}

fn find_driver() -> Result<PathBuf, AppError> {
    let executable = env::current_exe().map_err(AppError::CurrentExecutable)?;
    let executable_dir = executable
        .parent()
        .expect("the current executable always has a parent directory");

    let mut candidate_dirs = vec![executable_dir.to_owned()];
    if executable_dir.file_name().and_then(|name| name.to_str()) == Some("deps")
        && let Some(parent) = executable_dir.parent()
    {
        candidate_dirs.push(parent.to_owned());
    }

    for directory in candidate_dirs {
        for name in ["mlua-typegen-driver", "mlua-typegen-driver.exe"] {
            let driver = directory.join(name);
            if driver.is_file() {
                return Ok(driver);
            }
        }
    }

    Ok(PathBuf::from("mlua-typegen-driver"))
}

fn find_sysroot_lib_dir(toolchain: &'static str) -> Result<Option<PathBuf>, AppError> {
    let output = Command::new("rustup")
        .args(["run", toolchain, "rustc", "--print", "sysroot"])
        .output()
        .map_err(|source| AppError::QuerySysroot { toolchain, source })?;
    if !output.status.success() {
        return Err(AppError::SysrootQueryFailed {
            toolchain,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let sysroot = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok([sysroot.join("bin"), sysroot.join("lib")]
        .into_iter()
        .find(|directory| directory.is_dir()))
}

fn prepend_lib_path(command: &mut Command, directory: &Path) -> Result<(), AppError> {
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
        env::join_paths(paths).map_err(AppError::LibraryPath)?,
    );
    Ok(())
}

fn stage_driver_runtime(driver: &Path, sysroot_lib: &Path) -> Result<(), AppError> {
    if !cfg!(windows) || !driver.is_file() {
        return Ok(());
    }
    let Some(driver_dir) = driver.parent() else {
        return Ok(());
    };
    let entries = fs::read_dir(sysroot_lib).map_err(|source| AppError::ReadRuntime {
        path: sysroot_lib.to_owned(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| AppError::ReadRuntime {
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
                return Err(AppError::ReadRuntime {
                    path: destination,
                    source: error,
                });
            }
            (Err(error), _) => {
                return Err(AppError::ReadRuntime {
                    path: source_path,
                    source: error,
                });
            }
        };
        if needs_copy {
            fs::copy(&source_path, &destination).map_err(|source| AppError::CopyRuntime {
                source_path,
                destination,
                source,
            })?;
        }
    }
    Ok(())
}

fn pass_through_env(command: &mut Command, name: &str) {
    if let Some(value) = env::var_os(name) {
        command.env(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_generator_options_without_consuming_cargo_arguments() {
        let options = CliOptions::parse([
            OsString::from("--emmylua"),
            OsString::from("--output"),
            OsString::from("generated"),
            OsString::from("--workspace"),
        ])
        .unwrap();

        assert_eq!(options.output_dir, PathBuf::from("generated"));
        assert!(options.emit_emmylua);
        assert_eq!(options.cargo_args, [OsString::from("--workspace")]);
    }

    #[test]
    fn rejects_output_without_a_directory() {
        let error = CliOptions::parse([OsString::from("--output")]).unwrap_err();
        assert!(matches!(error, AppError::MissingOutput));
    }
}
