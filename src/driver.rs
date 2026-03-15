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
        let api = extract::collect_lua_api(tcx);

        let total = api.classes.len()
            + api.enums.len()
            + api.modules.len()
            + api.global_functions.len();

        if total > 0 {
            let crate_name = tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE);
            let crate_name = crate_name.as_str();
            if let Err(e) = mlua_typegen::codegen::write_stubs_for(&self.output_dir, &api, self.target, crate_name) {
                eprintln!("mlua-typegen: failed to write stubs: {e}");
            } else {
                eprintln!(
                    "mlua-typegen: generated stubs ({} classes, {} enums, {} modules, {} globals) in {}/{}",
                    api.classes.len(),
                    api.enums.len(),
                    api.modules.len(),
                    api.global_functions.len(),
                    self.output_dir.display(),
                    crate_name,
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

        let target = if std::env::var("MLUA_TYPEGEN_EMMYLUA").is_ok() {
            CodegenTarget::EmmyLua
        } else {
            CodegenTarget::LuaLS
        };

        let mut callbacks = LuaTypegenCallbacks { output_dir, target };

        let compiler_args: Vec<String> = std::iter::once(rustc)
            .chain(rustc_args.iter().cloned())
            .collect();

        return rustc_driver::catch_with_exit_code(|| {
            rustc_driver::run_compiler(&compiler_args, &mut callbacks)
        });
    }

    // Direct invocation mode (not as wrapper)
    let output_dir = extract_flag(&mut args, "--mlua-typegen-output=");
    let emmylua = extract_bool_flag(&mut args, "--mlua-typegen-emmylua");

    let output_dir = output_dir
        .map(PathBuf::from)
        .or_else(|| std::env::var("MLUA_TYPEGEN_OUTPUT").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("lua-types"));

    let target = if emmylua || std::env::var("MLUA_TYPEGEN_EMMYLUA").is_ok() {
        CodegenTarget::EmmyLua
    } else {
        CodegenTarget::LuaLS
    };

    let mut callbacks = LuaTypegenCallbacks { output_dir, target };

    rustc_driver::catch_with_exit_code(|| {
        rustc_driver::run_compiler(&args[1..], &mut callbacks)
    })
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
