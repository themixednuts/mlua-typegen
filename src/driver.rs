#![feature(rustc_private)]

extern crate rustc_abi;
extern crate rustc_ast;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod extract;

use std::path::PathBuf;

use mlua_typegen::{
    Dialect,
    config::{self, ResolvedConfig},
    driver_protocol::{
        EVIDENCE_SNAPSHOT_ENV, EXTRACTION_SCOPE_ENV, EvidenceCatalog, FORCE_ANALYZE_ENV,
        INVOCATION_LEDGER_ENV, InvocationRecord, REPLAY_GENERATION_ENV, UnitManifest,
    },
};
use rustc_driver::Callbacks;
use rustc_interface::interface;
use rustc_middle::ty::TyCtxt;

struct LuaTypegenCallbacks {
    output_dir: PathBuf,
    dialect: Dialect,
    config: ResolvedConfig,
    evidence: EvidenceCatalog,
    scope: Option<std::collections::BTreeSet<String>>,
    generation: u32,
    compiler_args: Vec<String>,
}

impl Callbacks for LuaTypegenCallbacks {
    fn after_analysis(
        &mut self,
        _compiler: &interface::Compiler,
        tcx: TyCtxt<'_>,
    ) -> rustc_driver::Compilation {
        let crate_name = tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE);
        let crate_name = crate_name.as_str();
        record_invocation(tcx, crate_name, &self.compiler_args);
        let compiled = match extract::extract_unit(
            tcx,
            &self.config,
            &self.evidence,
            self.scope.as_ref(),
        ) {
            Ok(compiled) => compiled,
            Err(diagnostics) => {
                emit_compile_diagnostics(tcx, &diagnostics);
                eprintln!(
                    "mlua-typegen: incomplete run for `{crate_name}`; published no compiler report"
                );
                return rustc_driver::Compilation::Stop;
            }
        };

        if std::env::var("MLUA_TYPEGEN_TRACE").is_ok() {
            eprintln!("[mlua-typegen] driver crate={crate_name}");
            eprintln!(
                "[mlua-typegen] resolved explicit mappings={}",
                self.config.mappings().len()
            );
            eprintln!(
                "[mlua-typegen] semantic API items={}",
                compiled.unit.api.item_count()
            );
            for conversion in &compiled.unit.conversions {
                eprintln!("[mlua-typegen] driver conversion={conversion:?}");
            }
        }

        let report = mlua_typegen::driver_protocol::write_unit_report(
            &self.output_dir,
            &compiled.unit,
            &compiled.used_mappings,
            &UnitManifest {
                stable_crate_id: tcx.stable_crate_id(rustc_hir::def_id::LOCAL_CRATE).as_u64(),
                crate_name: crate_name.to_owned(),
                generation: self.generation,
                scope: self.scope.clone(),
                pending: !compiled.requirements.is_empty(),
                requirements: compiled.requirements,
                provided_classes: compiled.provided_classes,
                conversions: compiled.unit.conversions.clone(),
            },
            self.dialect,
            crate_name,
        )
        .unwrap_or_else(|error| {
            tcx.dcx().fatal(format!(
                "mlua-typegen failed to commit compiler report for `{crate_name}`: {error}"
            ))
        });
        eprintln!(
            "mlua-typegen: committed {} extracted items from {} to {}",
            report.item_count,
            report.crate_name,
            report.path.display()
        );

        rustc_driver::Compilation::Continue
    }
}

fn emit_compile_diagnostics(tcx: TyCtxt<'_>, diagnostics: &extract::CompileDiagnostics) {
    for diagnostic in diagnostics.iter() {
        let mut rendered = tcx.dcx().struct_span_err(
            diagnostic.primary_span(),
            format!(
                "[{}] mlua-typegen could not prove an exact Lua type",
                diagnostic.code()
            ),
        );
        rendered.note(format!("stage: {}", diagnostic.stage()));
        rendered.note(format!("Rust owner: `{}`", diagnostic.rust_owner()));
        if let Some(lua_path) = diagnostic.lua_path() {
            rendered.note(format!("Lua-visible path: `{lua_path}`"));
        }
        rendered.note(format!("reason: {}", diagnostic.reason()));
        rendered.help(diagnostic.remediation());
        rendered.emit();
    }
}

fn main() -> std::process::ExitCode {
    let mut args: Vec<String> = std::env::args().collect();

    // When used as RUSTC_WRAPPER, cargo passes: <wrapper> <rustc> <args...>
    // Detect this by checking if arg[1] looks like a rustc binary
    let is_wrapper = args.get(1).is_some_and(|a| {
        let p = std::path::Path::new(a);
        p.file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s == "rustc")
    });

    if is_wrapper {
        let rustc = args[1].clone();
        let rustc_args = &args[2..];

        let output_dir = std::env::var("MLUA_TYPEGEN_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("lua-types"));

        let dialect = codegen_dialect(false);

        let compiler_args = make_compiler_args(rustc, rustc_args);
        let force_analyze = std::env::var_os(FORCE_ANALYZE_ENV).is_some();
        let primary = std::env::var_os("CARGO_PRIMARY_PACKAGE").is_some();
        if !primary && !force_analyze {
            let mut callbacks = RecordingCallbacks {
                compiler_args: compiler_args.clone(),
            };
            return run_compiler_with_exit_code(&compiler_args, &mut callbacks);
        }

        let config = match load_config_snapshot() {
            Ok(config) => config,
            Err(error) => {
                eprintln!("error: {error}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let mut callbacks = LuaTypegenCallbacks {
            output_dir,
            dialect,
            config,
            evidence: load_evidence_snapshot(),
            scope: load_extraction_scope(),
            generation: replay_generation(),
            compiler_args: compiler_args.clone(),
        };

        return run_compiler_with_exit_code(&compiler_args, &mut callbacks);
    }

    // Direct invocation mode (not as wrapper)
    let output_dir = extract_flag(&mut args, "--mlua-typegen-output=");
    let emit_emmylua = extract_bool_flag(&mut args, "--mlua-typegen-emmylua");

    let output_dir = output_dir
        .map(PathBuf::from)
        .or_else(|| std::env::var("MLUA_TYPEGEN_OUTPUT").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("lua-types"));

    let dialect = codegen_dialect(emit_emmylua);

    let config = match load_config_snapshot() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let compiler_args = if args.len() > 1 {
        make_compiler_args(args[1].clone(), &args[2..])
    } else {
        vec!["rustc".to_string()]
    };
    let mut callbacks = LuaTypegenCallbacks {
        output_dir,
        dialect,
        config,
        evidence: load_evidence_snapshot(),
        scope: load_extraction_scope(),
        generation: replay_generation(),
        compiler_args: compiler_args.clone(),
    };

    run_compiler_with_exit_code(&compiler_args, &mut callbacks)
}

struct RecordingCallbacks {
    compiler_args: Vec<String>,
}

impl Callbacks for RecordingCallbacks {
    fn after_analysis(
        &mut self,
        _compiler: &interface::Compiler,
        tcx: TyCtxt<'_>,
    ) -> rustc_driver::Compilation {
        let crate_name = tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE);
        record_invocation(tcx, crate_name.as_str(), &self.compiler_args);
        rustc_driver::Compilation::Continue
    }
}

fn record_invocation(tcx: TyCtxt<'_>, crate_name: &str, compiler_args: &[String]) {
    let Some(root) = std::env::var_os(INVOCATION_LEDGER_ENV).map(PathBuf::from) else {
        return;
    };
    let stable_crate_id = tcx.stable_crate_id(rustc_hir::def_id::LOCAL_CRATE).as_u64();
    let record = InvocationRecord {
        stable_crate_id,
        crate_name: crate_name.to_owned(),
        compiler_args: compiler_args.to_vec(),
    };
    let bytes = serde_json::to_vec(&record).expect("invocation record is serializable");
    let temporary = root.join(format!("{stable_crate_id:016x}.{}.tmp", std::process::id()));
    let destination = root.join(format!("{stable_crate_id:016x}.json"));
    if std::fs::write(&temporary, bytes).is_ok() {
        let _ = std::fs::rename(&temporary, destination);
    }
}

fn load_evidence_snapshot() -> EvidenceCatalog {
    let Some(path) = std::env::var_os(EVIDENCE_SNAPSHOT_ENV) else {
        return EvidenceCatalog::default();
    };
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn load_extraction_scope() -> Option<std::collections::BTreeSet<String>> {
    let path = std::env::var_os(EXTRACTION_SCOPE_ENV)?;
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn replay_generation() -> u32 {
    std::env::var(REPLAY_GENERATION_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn load_config_snapshot() -> Result<ResolvedConfig, String> {
    let Some(path) = std::env::var_os(config::CONFIG_SNAPSHOT_ENV) else {
        return Ok(ResolvedConfig::empty());
    };
    ResolvedConfig::from_snapshot(std::path::Path::new(&path)).map_err(|error| error.to_string())
}

#[cfg(rustc_driver_catch_returns_exit_code)]
fn run_compiler_with_exit_code<C: Callbacks + Send>(
    compiler_args: &[String],
    callbacks: &mut C,
) -> std::process::ExitCode {
    rustc_driver::catch_with_exit_code(|| rustc_driver::run_compiler(compiler_args, callbacks))
}

#[cfg(not(rustc_driver_catch_returns_exit_code))]
fn run_compiler_with_exit_code<C: Callbacks + Send>(
    compiler_args: &[String],
    callbacks: &mut C,
) -> std::process::ExitCode {
    std::process::ExitCode::from(rustc_driver::catch_with_exit_code(|| {
        rustc_driver::run_compiler(compiler_args, callbacks)
    }) as u8)
}

fn make_compiler_args(rustc: String, rustc_args: &[String]) -> Vec<String> {
    let mut compiler_args = Vec::with_capacity(rustc_args.len() + 3);
    compiler_args.push(rustc.clone());
    compiler_args.extend(rustc_args.iter().cloned());

    let has_sysroot = rustc_args.iter().any(|arg| arg == "--sysroot")
        || rustc_args.iter().any(|arg| arg.starts_with("--sysroot="));

    if !has_sysroot && let Some(sysroot) = infer_sysroot_from_rustc(&rustc) {
        compiler_args.push("--sysroot".to_string());
        compiler_args.push(sysroot);
    }

    compiler_args
}

fn infer_sysroot_from_rustc(rustc: &str) -> Option<String> {
    let rustc = std::path::Path::new(rustc);
    if let (Some(bin), true) = (rustc.parent(), rustc.is_absolute())
        && let Some(sysroot) = bin.parent()
    {
        return Some(sysroot.to_string_lossy().into_owned());
    }

    let mut cmd = if let Ok(toolchain) = std::env::var("RUSTUP_TOOLCHAIN") {
        let mut cmd = std::process::Command::new("rustup");
        cmd.args(["run", &toolchain, "rustc", "--print", "sysroot"]);
        cmd
    } else {
        let mut cmd = std::process::Command::new(rustc);
        cmd.args(["--print", "sysroot"]);
        cmd
    };

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let sysroot = String::from_utf8(output.stdout).ok()?;
    let sysroot = sysroot.trim();
    (!sysroot.is_empty()).then(|| sysroot.to_string())
}

fn codegen_dialect(force_emmylua: bool) -> Dialect {
    if force_emmylua || std::env::var("MLUA_TYPEGEN_EMMYLUA").is_ok() {
        Dialect::EmmyLua
    } else {
        Dialect::LuaLs
    }
}

/// Extract a `--key=value` flag from args, removing it so rustc doesn't see it.
fn extract_flag(args: &mut Vec<String>, prefix: &str) -> Option<String> {
    let mut value = None;
    args.retain(|arg| {
        if let Some(v) = arg.strip_prefix(prefix) {
            value = Some(v.to_string());
            false
        } else {
            true
        }
    });
    value
}

/// Extract a boolean flag from args, removing it so rustc doesn't see it.
fn extract_bool_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let mut found = false;
    args.retain(|arg| {
        if arg == flag {
            found = true;
            false
        } else {
            true
        }
    });
    found
}
