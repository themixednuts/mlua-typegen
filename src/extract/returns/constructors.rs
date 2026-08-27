use mlua_typegen::LuaType;
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use super::super::{is_passthrough_method, type_display_name};

pub(in crate::extract) fn lua_value_variant_to_type(variant_name: &str) -> Option<LuaType> {
    match variant_name {
        "Nil" => Some(LuaType::Nil),
        "Boolean" => Some(LuaType::Boolean),
        "Integer" => Some(LuaType::Integer),
        "Number" => Some(LuaType::Number),
        "String" => Some(LuaType::String),
        "LightUserData" => Some(LuaType::Any),
        "Table" => Some(LuaType::Table),
        "Function" => Some(LuaType::Function),
        "Thread" => Some(LuaType::Thread),
        _ => None, // UserData, Error — can't narrow further
    }
}

/// Check if an expression is a `LuaValue::Variant(...)` or `Value::Variant(...)` constructor
/// and return the corresponding LuaType.
pub(in crate::extract) fn try_lua_value_constructor<'tcx>(
    tcx: TyCtxt<'tcx>,
    typeck: &'tcx ty::TypeckResults<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    match &expr.kind {
        // LuaValue::Nil (path expression, no call)
        hir::ExprKind::Path(qpath) => {
            let variant = match qpath {
                hir::QPath::Resolved(_, path) => {
                    path.segments.last().map(|seg| seg.ident.name.as_str())
                }
                hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
            }?;

            let ty = typeck.expr_ty(expr);
            if let ty::TyKind::Adt(adt_def, _) = ty.kind() {
                let path_str = tcx.def_path_str(adt_def.did());
                if path_str.contains("Value") && !path_str.contains("MultiValue") {
                    return lua_value_variant_to_type(variant);
                }
            }
            None
        }
        // LuaValue::String(x), LuaValue::Integer(x), etc. (call expression)
        hir::ExprKind::Call(callee, _) => {
            if let hir::ExprKind::Path(hir::QPath::TypeRelative(_, seg)) = &callee.kind {
                return lua_value_variant_to_type(seg.ident.name.as_str());
            }
            if let hir::ExprKind::Path(hir::QPath::Resolved(_, path)) = &callee.kind
                && let Some(seg) = path.segments.last()
            {
                // Verify the return type is a Value type
                let ty = typeck.expr_ty(expr);
                if let ty::TyKind::Adt(adt_def, _) = ty.kind() {
                    let path_str = tcx.def_path_str(adt_def.did());
                    if path_str.contains("Value") && !path_str.contains("MultiValue") {
                        return lua_value_variant_to_type(seg.ident.name.as_str());
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract a type name from a QPath (e.g. `TypeName::make`)
pub(in crate::extract) fn extract_type_name_from_qpath(qpath: &hir::QPath<'_>) -> Option<String> {
    let name = match qpath {
        hir::QPath::TypeRelative(ty, _) => {
            if let hir::TyKind::Path(hir::QPath::Resolved(_, path)) = &ty.kind {
                path.segments
                    .last()
                    .map(|s| s.ident.name.as_str().to_string())
            } else {
                None
            }
        }
        hir::QPath::Resolved(_, path) => {
            let last = path.segments.last()?.ident.name.as_str().to_string();
            let last_starts_like_type = last.chars().next().is_some_and(|c| c.is_uppercase());

            if last_starts_like_type || path.segments.len() == 1 {
                Some(last)
            } else if path.segments.len() >= 2 {
                Some(
                    path.segments[path.segments.len() - 2]
                        .ident
                        .name
                        .as_str()
                        .to_string(),
                )
            } else {
                None
            }
        }
    }?;

    let starts_like_type = name.chars().next().is_some_and(|c| c.is_uppercase());

    if matches!(name.as_str(), "Ok" | "Err" | "Some" | "None")
        || name.starts_with("__")
        || (!starts_like_type && name != "Self")
    {
        None
    } else {
        Some(name)
    }
}

/// Extract the return type name from a `fn(...) -> Result<TypeName>` cast expression.
/// Uses typeck to get the actual return type of the call expression, then
/// checks if the cast target (the $value closure) body reveals the type.
pub(in crate::extract) fn extract_return_type_from_cast<'tcx>(
    tcx: TyCtxt<'tcx>,
    cast_expr: &'tcx hir::Expr<'tcx>,
    _cast_ty: &hir::Ty<'_>,
) -> Option<String> {
    // The cast expression is typically a closure literal like `|_, me| Mode::make(&me.mode)`
    // Try to extract the type from its body
    extract_type_from_closure_expr(tcx, cast_expr)
}

pub(in crate::extract) fn extract_type_name_from_value_path_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<String> {
    let hir::ExprKind::Path(qpath) = &expr.kind else {
        return None;
    };

    match qpath {
        hir::QPath::TypeRelative(_, _) => extract_type_name_from_qpath(qpath),
        hir::QPath::Resolved(_, _) => {
            let res = tcx
                .typeck(expr.hir_id.owner.def_id)
                .qpath_res(qpath, expr.hir_id);
            matches!(
                res,
                rustc_hir::def::Res::Def(
                    rustc_hir::def::DefKind::Ctor(rustc_hir::def::CtorOf::Struct, _),
                    _
                )
            )
            .then(|| extract_type_name_from_qpath(qpath))
            .flatten()
        }
    }
}

/// Try to extract a class name from a closure expression by looking at what it calls.
/// Recursively searches through closures, blocks, method chains, and call expressions.
pub(in crate::extract) fn extract_type_from_closure_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<String> {
    match &expr.kind {
        hir::ExprKind::DropTemps(inner)
        | hir::ExprKind::Use(inner, _)
        | hir::ExprKind::Cast(inner, _)
        | hir::ExprKind::Type(inner, _)
        | hir::ExprKind::AddrOf(_, _, inner)
        | hir::ExprKind::Unary(_, inner)
        | hir::ExprKind::Field(inner, _)
        | hir::ExprKind::Become(inner)
        | hir::ExprKind::Yield(inner, _)
        | hir::ExprKind::UnsafeBinderCast(_, inner, _) => {
            extract_type_from_closure_expr(tcx, inner)
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            extract_type_from_closure_expr(tcx, body.value)
        }
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr
                && let Some(name) = extract_type_from_closure_expr(tcx, tail)
            {
                return Some(name);
            }
            // Check statements too
            for stmt in block.stmts {
                if let hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) = &stmt.kind
                    && let Some(name) = extract_type_from_closure_expr(tcx, e)
                {
                    return Some(name);
                }
            }
            None
        }
        hir::ExprKind::Call(callee, args) => {
            if let Some(name) = extract_type_name_from_value_path_expr(tcx, callee) {
                return Some(name);
            }
            // Check args (e.g. the closure in .map(|x| TypeName::make(x)))
            for arg in *args {
                if let Some(name) = extract_type_from_closure_expr(tcx, arg) {
                    return Some(name);
                }
            }
            None
        }
        // Method chains like .map(|x| TypeName::make(x)).transpose()
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            // Check receiver (the chain before this method)
            if let Some(name) = extract_type_from_closure_expr(tcx, receiver) {
                return Some(name);
            }
            // Check args (closures passed to .map(), .and_then(), etc.)
            for arg in *args {
                if let Some(name) = extract_type_from_closure_expr(tcx, arg) {
                    return Some(name);
                }
            }
            None
        }
        hir::ExprKind::Match(scrut, arms, _) => {
            if let Some(name) = extract_type_from_closure_expr(tcx, scrut) {
                return Some(name);
            }
            for arm in *arms {
                if let Some(name) = extract_type_from_closure_expr(tcx, arm.body) {
                    return Some(name);
                }
            }
            None
        }
        // If/else — check both branches
        hir::ExprKind::If(_, then_expr, else_expr) => {
            if let Some(name) = extract_type_from_closure_expr(tcx, then_expr) {
                return Some(name);
            }
            if let Some(e) = else_expr {
                extract_type_from_closure_expr(tcx, e)
            } else {
                None
            }
        }
        // Bare function reference like Filter::make — extract the type from the path
        hir::ExprKind::Path(_) => extract_type_name_from_value_path_expr(tcx, expr),
        _ => None,
    }
}

/// Try to resolve the class name from an expression that produces AnyUserData.
/// Handles patterns like:
///   - `TypeName::make(...)` — associated function call where TypeName is a UserData class
///   - `lua.create_any_userdata(TypeName { ... })` — direct creation
///   - `expr?` (Result unwrapping via ? operator, desugared to Match)
pub(in crate::extract) fn resolve_any_user_data_class<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    match &expr.kind {
        // TypeName::method(...) — check if TypeName is a known class
        hir::ExprKind::Call(callee, args) => {
            // Check callee path for TypeName::make(...) pattern
            let type_name = match &callee.kind {
                hir::ExprKind::Path(qpath) => extract_type_name_from_qpath(qpath),
                // ($value as fn(&Lua, &Self) -> Result<_>)(lua, me) — cast expr
                hir::ExprKind::Cast(inner, cast_ty) => {
                    extract_return_type_from_cast(tcx, inner, cast_ty)
                }
                _ => None,
            };
            if let Some(name) = type_name {
                return Some(LuaType::Class(name));
            }
            // Recurse into call args
            for arg in *args {
                if let Some(ty) = resolve_any_user_data_class(tcx, arg) {
                    return Some(ty);
                }
            }
            None
        }
        // expr? desugars to Match — recurse into the Ok arm
        hir::ExprKind::Match(scrut, arms, _) => {
            resolve_any_user_data_class(tcx, scrut).or_else(|| {
                arms.iter()
                    .find_map(|arm| resolve_any_user_data_class(tcx, arm.body))
            })
        }
        // expr.method()? or similar chains
        // Also handles lua.create_any_userdata(concrete_value) — trace arg type
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let method_name = segment.ident.name.as_str();
            if method_name == "create_any_userdata" && !args.is_empty() {
                let typeck = tcx.typeck(args[0].hir_id.owner.def_id);
                let arg_ty = typeck.expr_ty(&args[0]);
                if let ty::TyKind::Adt(..) = arg_ty.kind() {
                    let name = type_display_name(tcx, arg_ty);
                    return Some(LuaType::Class(name));
                }
            }
            if is_passthrough_method(method_name) {
                resolve_any_user_data_class(tcx, receiver)
            } else {
                args.iter()
                    .find_map(|arg| resolve_any_user_data_class(tcx, arg))
            }
        }
        // Block — check tail expression
        hir::ExprKind::Block(block, _) => {
            block.expr.and_then(|e| resolve_any_user_data_class(tcx, e))
        }
        // Closure — cross boundary
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            resolve_any_user_data_class(tcx, body.value)
        }
        _ => None,
    }
}
