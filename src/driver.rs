#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod extract;

use std::path::PathBuf;

use mlua_typegen::CodegenTarget;
use rustc_driver::Callbacks;
use rustc_interface::interface;
use rustc_middle::ty::TyCtxt;

struct LuaTypegenCallbacks {
    output_dir: PathBuf,
    target: CodegenTarget,
}

impl Callbacks for LuaTypegenCallbacks {
    fn after_analysis(
        &mut self,
        _compiler: &interface::Compiler,
        tcx: TyCtxt<'_>,
    ) -> rustc_driver::Compilation {
        let crate_name = tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE);
        let crate_name = crate_name.as_str();
        let api = extract::collect_lua_api(tcx);

        if std::env::var("MLUA_TYPEGEN_TRACE").is_ok() {
            eprintln!("[mlua-typegen] driver crate={crate_name}");
            for class in &api.classes {
                let interesting: Vec<_> = class
                    .methods
                    .iter()
                    .filter(|m| {
                        matches!(
                            m.name.as_str(),
                            "open"
                                | "history"
                                | "ends_with"
                                | "join"
                                | "starts_with"
                                | "strip_prefix"
                                | "raw"
                                | "__eq"
                                | "__pairs"
                        )
                    })
                    .map(|m| format!("{} params={:?} returns={:?}", m.name, m.params, m.returns))
                    .collect();
                if !interesting.is_empty()
                    || matches!(
                        class.name.as_str(),
                        "Access" | "Path" | "Url" | "Style" | "File" | "Tab"
                    )
                {
                    eprintln!(
                        "[mlua-typegen] driver class={} fields={:?} methods={:?}",
                        class.name, class.fields, interesting
                    );
                }
            }
            for func in &api.global_functions {
                if matches!(func.name.as_str(), "Url" | "Path" | "File") {
                    eprintln!(
                        "[mlua-typegen] driver global_function={} params={:?} returns={:?}",
                        func.name, func.params, func.returns
                    );
                }
            }
        }

        let total = api.classes.len()
            + api.enums.len()
            + api.modules.len()
            + api.global_fields.len()
            + api.global_functions.len();

        if total > 0 {
            if let Err(e) = mlua_typegen::codegen::write_stubs_for(
                &self.output_dir,
                &api,
                self.target,
                crate_name,
            ) {
                eprintln!("mlua-typegen: failed to write stubs: {e}");
            } else {
                eprintln!(
                    "mlua-typegen: generated stubs ({} classes, {} enums, {} modules, {} global values, {} global functions) in {}/{}",
                    api.classes.len(),
                    api.enums.len(),
                    api.modules.len(),
                    api.global_fields.len(),
                    api.global_functions.len(),
                    self.output_dir.display(),
                    crate_name,
                );
            }
        }

        // Write event emissions to shared file for cross-crate callback inference
        if !api.event_emissions.is_empty() {
            if let Err(e) = mlua_typegen::codegen::append_event_emissions(
                &self.output_dir,
                &api.event_emissions,
            ) {
                eprintln!("mlua-typegen: failed to write events: {e}");
            } else {
                eprintln!(
                    "mlua-typegen: recorded {} event emissions from {crate_name}",
                    api.event_emissions.len()
                );
            }
        }

        rustc_driver::Compilation::Continue
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

        // RUSTC_WORKSPACE_WRAPPER only sends workspace crates through us,
        // so every invocation here is a crate we should analyze.
        let output_dir = std::env::var("MLUA_TYPEGEN_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("lua-types"));

        let target = codegen_target(false);

        let mut callbacks = LuaTypegenCallbacks { output_dir, target };

        let compiler_args = make_compiler_args(rustc, rustc_args);

        return rustc_driver::catch_with_exit_code(|| {
            rustc_driver::run_compiler(&compiler_args, &mut callbacks)
        });
    }

    // Direct invocation mode (not as wrapper)
    let output_dir = extract_flag(&mut args, "--mlua-typegen-output=");
    let emit_emmylua = extract_bool_flag(&mut args, "--mlua-typegen-emmylua");

    let output_dir = output_dir
        .map(PathBuf::from)
        .or_else(|| std::env::var("MLUA_TYPEGEN_OUTPUT").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("lua-types"));

    let target = codegen_target(emit_emmylua);

    let mut callbacks = LuaTypegenCallbacks { output_dir, target };
    let compiler_args = if args.len() > 1 {
        make_compiler_args(args[1].clone(), &args[2..])
    } else {
        vec!["rustc".to_string()]
    };

    rustc_driver::catch_with_exit_code(|| {
        rustc_driver::run_compiler(&compiler_args, &mut callbacks)
    })
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

fn codegen_target(force_emmylua: bool) -> CodegenTarget {
    if force_emmylua || std::env::var("MLUA_TYPEGEN_EMMYLUA").is_ok() {
        CodegenTarget::EmmyLua
    } else {
        CodegenTarget::LuaLS
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
