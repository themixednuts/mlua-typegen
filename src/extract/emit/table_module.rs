use mlua_typegen::LuaModule;
use rustc_hir as hir;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::TyCtxt;

use crate::extract::{
    expr_def_id, expr_snippet, extract_string_literal, path_tail_name, peel_lua_conversion_expr,
    trace, unwrap_try_expr,
};

use super::function_value::try_extract_function_value;
use super::registration::nested_module_name;
use super::value_inference::{ExtractedModule, classify_exported_value, push_exported_value};

pub(in crate::extract) fn create_table_module_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module_name: &str,
) -> Option<ExtractedModule> {
    let expr = peel_lua_conversion_expr(expr);
    let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind else {
        return None;
    };

    let method = segment.ident.name.as_str();
    if method != "create_table" && method != "create_table_from" {
        return None;
    }

    let mut module = LuaModule {
        name: module_name.to_string(),
        doc: None,
        functions: Vec::new(),
        fields: Vec::new(),
    };
    let mut nested_modules = Vec::new();

    if method == "create_table_from" && !args.is_empty() {
        populate_module_from_table_entries(
            tcx,
            &args[0],
            module_name,
            &mut module,
            &mut nested_modules,
        );
    }

    Some(ExtractedModule {
        module,
        nested_modules,
    })
}

/// Detect calls like `get_or_create_module(lua, "wezterm")` or
/// `get_or_create_sub_module(lua, "color")` and extract the module name
/// from the string literal argument.
pub(in crate::extract) fn try_extract_module_name_from_call<'tcx>(
    _tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<String> {
    let expr = peel_lua_conversion_expr(expr);

    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            // Free function call: get_or_create_module(lua, "name")
            let callee_name = path_tail_name(callee)?;
            if !callee_name.contains("module") {
                return None;
            }
            // Extract the string literal argument (usually the last one).
            args.iter().rev().find_map(extract_string_literal)
        }
        hir::ExprKind::MethodCall(segment, _receiver, args, _) => {
            // Method call: lua.create_named_module("name") or similar
            let method = segment.ident.name.as_str();
            if !method.contains("module") {
                return None;
            }
            args.iter().rev().find_map(extract_string_literal)
        }
        _ => None,
    }
}

pub(in crate::extract) fn populate_module_from_table_entries<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module_name: &str,
    module: &mut LuaModule,
    nested_modules: &mut Vec<LuaModule>,
) {
    let hir::ExprKind::Array(entries) = &expr.kind else {
        return;
    };

    for entry in *entries {
        let hir::ExprKind::Tup(parts) = &entry.kind else {
            continue;
        };
        if parts.len() != 2 {
            continue;
        }

        let Some(name) = extract_string_literal(&parts[0]) else {
            continue;
        };

        let exported = classify_exported_value(
            tcx,
            &parts[1],
            &name,
            Some(&nested_module_name(module_name, &name)),
        );
        push_exported_value(module, name, exported, Some(nested_modules));
    }
}

pub(in crate::extract) fn try_extract_module_from_value_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module_name: &str,
) -> Option<ExtractedModule> {
    trace(format!(
        "try module {} from {}",
        module_name,
        expr_snippet(tcx, expr)
    ));
    if let Some(module) = create_table_module_from_expr(tcx, expr, module_name) {
        trace(format!("module {} from table expr", module.module.name));
        return Some(module);
    }

    let expr = peel_lua_conversion_expr(expr);
    let hir::ExprKind::Call(callee, _) = &expr.kind else {
        return None;
    };
    let def_id = expr_def_id(tcx, callee)?;
    let local = def_id.as_local()?;
    trace(format!("module {} from local fn {:?}", module_name, local));
    extract_module_from_local_fn(tcx, local, module_name)
}

pub(in crate::extract) fn extract_module_from_local_fn<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: LocalDefId,
    module_name: &str,
) -> Option<ExtractedModule> {
    trace(format!(
        "extract module {} from def {:?}",
        module_name, def_id
    ));
    let rustc_hir::Node::Item(item) = tcx.hir_node_by_def_id(def_id) else {
        trace("module def is not an item");
        return None;
    };
    let hir::ItemKind::Fn { body, .. } = item.kind else {
        return None;
    };
    let body = tcx.hir_body(body);
    extract_composer_module_from_expr(tcx, body.value, module_name)
}

pub(in crate::extract) fn extract_composer_module_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module_name: &str,
) -> Option<ExtractedModule> {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                if let hir::StmtKind::Item(item_id) = stmt.kind {
                    let item = tcx.hir_item(item_id);
                    if item
                        .kind
                        .ident()
                        .is_some_and(|ident| ident.name.as_str() == "get")
                    {
                        return extract_composer_get_module(tcx, item, module_name);
                    }
                }
            }
            block
                .expr
                .and_then(|tail| extract_composer_module_from_expr(tcx, tail, module_name))
        }
        _ => None,
    }
}

pub(in crate::extract) fn extract_composer_get_module<'tcx>(
    tcx: TyCtxt<'tcx>,
    item: &hir::Item<'tcx>,
    module_name: &str,
) -> Option<ExtractedModule> {
    let hir::ItemKind::Fn { body, .. } = item.kind else {
        return None;
    };
    let body = tcx.hir_body(body);
    let match_expr = find_match_expr(body.value)?;

    let mut module = LuaModule {
        name: module_name.to_string(),
        doc: None,
        functions: Vec::new(),
        fields: Vec::new(),
    };
    let mut nested_modules = Vec::new();

    let hir::ExprKind::Match(_, arms, _) = &match_expr.kind else {
        return None;
    };

    for arm in *arms {
        let Some(key) = extract_bytes_key_from_pat(tcx, arm.pat) else {
            continue;
        };
        let value_expr = unwrap_try_expr(arm.body);

        if let Some(func) = try_extract_function_value(tcx, value_expr, &key) {
            module.functions.push(func);
            continue;
        }

        let exported = classify_exported_value(
            tcx,
            value_expr,
            &key,
            Some(&nested_module_name(module_name, &key)),
        );
        push_exported_value(&mut module, key, exported, Some(&mut nested_modules));
    }

    Some(ExtractedModule {
        module,
        nested_modules,
    })
}

pub(in crate::extract) fn find_match_expr<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<&'tcx hir::Expr<'tcx>> {
    match &expr.kind {
        hir::ExprKind::Match(_, _, _) => Some(expr),
        hir::ExprKind::Block(block, _) => block.expr.and_then(find_match_expr),
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            find_match_expr(receiver).or_else(|| args.iter().find_map(find_match_expr))
        }
        hir::ExprKind::Call(callee, args) => {
            find_match_expr(callee).or_else(|| args.iter().find_map(find_match_expr))
        }
        hir::ExprKind::DropTemps(inner)
        | hir::ExprKind::Use(inner, _)
        | hir::ExprKind::Cast(inner, _)
        | hir::ExprKind::Type(inner, _)
        | hir::ExprKind::AddrOf(_, _, inner)
        | hir::ExprKind::Unary(_, inner)
        | hir::ExprKind::Field(inner, _)
        | hir::ExprKind::Become(inner)
        | hir::ExprKind::Yield(inner, _)
        | hir::ExprKind::UnsafeBinderCast(_, inner, _) => find_match_expr(inner),
        _ => None,
    }
}

pub(in crate::extract) fn extract_bytes_key_from_pat(
    tcx: TyCtxt<'_>,
    pat: &hir::Pat<'_>,
) -> Option<String> {
    // Byte string literal patterns (b"__pairs") — extract via source span
    // since the HIR representation for byte str patterns varies across nightly versions
    let snippet = tcx.sess.source_map().span_to_snippet(pat.span).ok()?;
    let snippet = snippet.trim();
    if snippet == "_" {
        return None;
    }
    let snippet = snippet.strip_prefix("b\"")?.strip_suffix('"')?;
    Some(snippet.to_string())
}
