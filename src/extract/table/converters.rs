use mlua_typegen::{LuaType, make_union};
use rustc_hir as hir;
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::{self, TyCtxt};

use super::super::{
    def_snippet, expr_def_id, infer_local_callable_param_type, is_informative,
    lua_type_from_extracted_name, map_ty_to_lua, path_tail_name, peel_expr, peel_try_expr,
    unwrap_try_expr,
};
use super::{
    expr_refers_to_binding, hir_ty_to_lua_type, qpath_last_name, typeck_method_generic_type,
};

pub(in crate::extract) fn json_like_lua_type() -> LuaType {
    make_union(vec![
        LuaType::Nil,
        LuaType::Boolean,
        LuaType::Number,
        LuaType::String,
        LuaType::Table,
    ])
}

pub(in crate::extract) fn called_path_name(expr: &hir::Expr<'_>) -> Option<String> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };
    qpath_last_name(qpath)
}

pub(in crate::extract) fn path_contains_segment(expr: &hir::Expr<'_>, needle: &str) -> bool {
    let expr = peel_expr(expr);
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return false;
    };

    match qpath {
        hir::QPath::Resolved(_, path) => path
            .segments
            .iter()
            .any(|segment| segment.ident.name.as_str() == needle),
        hir::QPath::TypeRelative(ty, seg) => {
            seg.ident.name.as_str() == needle
                || matches!(&ty.kind, hir::TyKind::Path(inner) if qpath_last_name(inner).as_deref() == Some(needle))
        }
    }
}

pub(in crate::extract) fn infer_param_type_from_local_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    callee: &'tcx hir::Expr<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    param_name: &str,
) -> Option<LuaType> {
    let arg_index = args
        .iter()
        .position(|arg| path_tail_name(arg).as_deref() == Some(param_name))?;
    let def_id = expr_def_id(tcx, callee)?.as_local()?;
    infer_local_callable_param_type(tcx, def_id, arg_index)
}

pub(in crate::extract) fn infer_param_type_from_local_method_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    param_name: &str,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(_, receiver, args, _) = &expr.kind else {
        return None;
    };

    let arg_index = args
        .iter()
        .position(|arg| path_tail_name(arg).as_deref() == Some(param_name))?;

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let method_local = resolve_local_method_def_id(tcx, typeck, expr, receiver)?;

    infer_local_callable_param_type(tcx, method_local, arg_index + 1)
}

pub(in crate::extract) fn resolve_local_method_def_id<'tcx>(
    tcx: TyCtxt<'tcx>,
    typeck: &'tcx ty::TypeckResults<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    receiver: &'tcx hir::Expr<'tcx>,
) -> Option<LocalDefId> {
    let def_id = typeck.type_dependent_def_id(expr.hir_id)?;

    def_id.as_local().or_else(|| {
        let receiver_ty = typeck.expr_ty(receiver);
        let ty::TyKind::Adt(adt_def, _) = receiver_ty.kind() else {
            return None;
        };

        let method_name = match &expr.kind {
            hir::ExprKind::MethodCall(segment, _, _, _) => segment.ident.name,
            _ => return None,
        };

        tcx.inherent_impls(adt_def.did())
            .iter()
            .find_map(|impl_def_id| {
                tcx.associated_items(*impl_def_id)
                    .in_definition_order()
                    .find(|item| item.ident(tcx).name == method_name)
                    .and_then(|item| item.def_id.as_local())
            })
    })
}

pub(in crate::extract) fn infer_param_type_from_converter_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    callee: &'tcx hir::Expr<'tcx>,
    args: &'tcx [hir::Expr<'tcx>],
    param_name: &str,
) -> Option<LuaType> {
    let arg_index = args
        .iter()
        .position(|arg| expr_refers_to_binding(arg, param_name))?;
    let def_id = expr_def_id(tcx, callee)?;
    let snippet = def_snippet(tcx, def_id)?;

    let mut tys = Vec::new();
    if arg_index > 0 || snippet.contains("Value") {
        tys.extend(infer_value_converter_types_from_snippet(&snippet));
    }
    if tys.is_empty() {
        None
    } else {
        Some(make_union(tys))
    }
}

/// HIR-based converter type inference: walk a function body for Value variant patterns,
/// `TypeId::of::<T>()` calls and `borrow::<T>()` method calls.
pub(in crate::extract) fn infer_value_converter_types_from_hir<'tcx>(
    tcx: TyCtxt<'tcx>,
    body_expr: &'tcx hir::Expr<'tcx>,
) -> Vec<LuaType> {
    let mut tys = Vec::new();
    collect_converter_types_from_hir(tcx, body_expr, &mut tys);
    tys
}

pub(in crate::extract) fn collect_converter_types_from_hir<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    tys: &mut Vec<LuaType>,
) {
    match &expr.kind {
        // Value::Boolean, Value::String, etc. in match arms
        hir::ExprKind::Path(qpath) => {
            if let Some(name) = match qpath {
                hir::QPath::Resolved(_, path) => {
                    let segs: Vec<_> = path
                        .segments
                        .iter()
                        .map(|s| s.ident.name.as_str())
                        .collect();
                    (segs.len() >= 2 && segs[segs.len() - 2] == "Value")
                        .then(|| segs.last().copied().unwrap_or_default())
                }
                _ => None,
            } {
                match name {
                    "Nil" => tys.push(LuaType::Nil),
                    "Boolean" => tys.push(LuaType::Boolean),
                    "Integer" => tys.push(LuaType::Integer),
                    "Number" => tys.push(LuaType::Number),
                    "String" => tys.push(LuaType::String),
                    "Table" => tys.push(LuaType::Table),
                    "Function" => tys.push(LuaType::Function),
                    "Thread" => tys.push(LuaType::Thread),
                    _ => {}
                }
            }
        }
        // TypeId::of::<T>() / take::<T>() / borrow::<T>() — extract generic type
        hir::ExprKind::MethodCall(segment, _, _, _) => {
            let method = segment.ident.name.as_str();
            if matches!(
                method,
                "take" | "borrow" | "borrow_mut" | "try_borrow" | "try_borrow_mut"
            ) && let Some(lua_ty) = typeck_method_generic_type(tcx, expr)
                && is_informative(&lua_ty)
            {
                tys.push(lua_ty);
            }
        }
        hir::ExprKind::Call(callee, _) => {
            // TypeId::of::<T>()
            if let Some(lua_ty) = typeck_call_generic_type(tcx, expr)
                && is_informative(&lua_ty)
            {
                tys.push(lua_ty);
            }
            collect_converter_types_from_hir(tcx, callee, tys);
        }
        // Recurse into blocks, matches, etc.
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                if let hir::StmtKind::Expr(e) | hir::StmtKind::Semi(e) = &stmt.kind {
                    collect_converter_types_from_hir(tcx, e, tys);
                }
                if let hir::StmtKind::Let(local) = &stmt.kind
                    && let Some(init) = local.init
                {
                    collect_converter_types_from_hir(tcx, init, tys);
                }
            }
            if let Some(tail) = block.expr {
                collect_converter_types_from_hir(tcx, tail, tys);
            }
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            collect_converter_types_from_hir(tcx, scrutinee, tys);
            for arm in *arms {
                // Check arm patterns for Value::Variant
                collect_value_variants_from_pat(arm.pat, tys);
                collect_converter_types_from_hir(tcx, arm.body, tys);
            }
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            collect_converter_types_from_hir(tcx, cond, tys);
            collect_converter_types_from_hir(tcx, then_expr, tys);
            if let Some(e) = else_expr {
                collect_converter_types_from_hir(tcx, e, tys);
            }
        }
        _ => {}
    }
}

/// Extract Value::Variant names from match arm patterns.
pub(in crate::extract) fn collect_value_variants_from_pat(
    pat: &hir::Pat<'_>,
    tys: &mut Vec<LuaType>,
) {
    match &pat.kind {
        hir::PatKind::TupleStruct(qpath, _, _) | hir::PatKind::Struct(qpath, _, _) => {
            if let hir::QPath::Resolved(_, path) = qpath {
                let segs: Vec<_> = path
                    .segments
                    .iter()
                    .map(|s| s.ident.name.as_str())
                    .collect();
                if segs.len() >= 2 && segs[segs.len() - 2] == "Value" {
                    match *segs.last().unwrap_or(&"") {
                        "Boolean" => tys.push(LuaType::Boolean),
                        "Integer" => tys.push(LuaType::Integer),
                        "Number" => tys.push(LuaType::Number),
                        "String" => tys.push(LuaType::String),
                        "Table" => tys.push(LuaType::Table),
                        "Function" => tys.push(LuaType::Function),
                        "Thread" => tys.push(LuaType::Thread),
                        _ => {}
                    }
                }
            }
        }
        hir::PatKind::Or(pats) => {
            for p in *pats {
                collect_value_variants_from_pat(p, tys);
            }
        }
        _ => {}
    }
}

pub(in crate::extract) fn infer_value_converter_types_from_snippet(snippet: &str) -> Vec<LuaType> {
    let mut tys = Vec::new();

    for (needle, ty) in [
        ("Value::Nil", LuaType::Nil),
        ("Value::Boolean", LuaType::Boolean),
        ("Value::Integer", LuaType::Integer),
        ("Value::Number", LuaType::Number),
        ("Value::String", LuaType::String),
        ("Value::Table", LuaType::Table),
        ("Value::Function", LuaType::Function),
        ("Value::Thread", LuaType::Thread),
    ] {
        if snippet.contains(needle) {
            tys.push(ty);
        }
    }

    let markers = [
        "TypeId::of::<",
        "take::<",
        "borrow::<",
        "borrow_mut::<",
        "try_borrow::<",
        "try_borrow_mut::<",
    ];
    for marker in markers {
        for name in extract_all_generic_type_names(snippet, marker) {
            tys.push(lua_type_from_extracted_name(&name));
        }
    }

    if tys.is_empty()
        && (snippet.contains("value_to_data(")
            || snippet.contains("from_value(")
            || snippet.contains("::from_lua(")
            || snippet.contains(" from_lua("))
    {
        tys.extend(match json_like_lua_type() {
            LuaType::Union(items) => items,
            ty => vec![ty],
        });
    }

    tys
}

pub(in crate::extract) fn is_type_id_call_on_binding(
    expr: &hir::Expr<'_>,
    binding_name: &str,
) -> bool {
    let expr = peel_try_expr(expr);
    matches!(
        &expr.kind,
        hir::ExprKind::MethodCall(segment, receiver, _, _)
            if segment.ident.name.as_str() == "type_id"
                && path_tail_name(receiver).as_deref() == Some(binding_name)
    )
}

pub(in crate::extract) fn extract_type_id_guard_lua_type(
    tcx: TyCtxt<'_>,
    guard: Option<&hir::Expr<'_>>,
) -> Option<LuaType> {
    let expr = peel_expr(unwrap_try_expr(guard?));
    let hir::ExprKind::Binary(op, lhs, rhs) = &expr.kind else {
        return None;
    };

    if op.node != hir::BinOpKind::Eq {
        return None;
    }

    extract_type_id_of_lua_type(tcx, lhs).or_else(|| extract_type_id_of_lua_type(tcx, rhs))
}

pub(in crate::extract) fn extract_type_id_of_lua_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::Call(callee, _) = &expr.kind else {
        return typeck_call_generic_type(tcx, expr);
    };
    let args = match &callee.kind {
        hir::ExprKind::Path(hir::QPath::TypeRelative(_, seg))
            if seg.ident.name.as_str() == "of" =>
        {
            seg.args?
        }
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => {
            let seg = path.segments.last()?;
            if seg.ident.name.as_str() != "of" {
                return None;
            }
            seg.args?
        }
        _ => return None,
    };

    if let Some(hir::GenericArg::Type(ty)) = args.args.first()
        && let Some(lua_ty) = hir_ty_to_lua_type(tcx, ty)
        && is_informative(&lua_ty)
    {
        return Some(lua_ty);
    }

    // Fallback: typeck
    typeck_call_generic_type(tcx, expr)
}

/// Resolve the first generic type argument of a function call via typeck.
pub(in crate::extract) fn typeck_call_generic_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
) -> Option<LuaType> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let args = typeck.node_args(expr.hir_id);
    for arg in args.iter() {
        if let Some(ty) = arg.as_type() {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if is_informative(&lua_ty) {
                return Some(lua_ty);
            }
        }
    }
    None
}

pub(in crate::extract) fn extract_all_generic_type_names(
    snippet: &str,
    marker: &str,
) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = snippet;

    while let Some(start) = rest.find(marker) {
        let tail = &rest[start + marker.len()..];
        let Some((name, consumed)) = extract_balanced_generic_arg(tail) else {
            break;
        };
        if let Some(name) = name
            .rsplit("::")
            .next()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            names.push(name.to_string());
        }
        rest = &tail[consumed..];
    }

    names
}

pub(in crate::extract) fn extract_balanced_generic_arg(input: &str) -> Option<(&str, usize)> {
    let mut depth = 0usize;

    for (idx, ch) in input.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                if depth == 0 {
                    return Some((input[..idx].trim(), idx + ch.len_utf8()));
                }
                depth -= 1;
            }
            _ => {}
        }
    }

    None
}
