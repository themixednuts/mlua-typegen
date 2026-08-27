use mlua_typegen::LuaReturn;
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use super::super::map_ty_to_lua;

/// Try to extract return-value names from the body's return tuple expression.
/// Works on `Ok((a.x, a.y, z))` and bare `(a.x, a.y, z)` patterns.
/// Enriches existing typed returns with names when the tuple element count matches.
pub(in crate::extract) fn enrich_return_names<'tcx>(
    _tcx: TyCtxt<'tcx>,
    body: &'tcx hir::Expr<'tcx>,
    returns: &mut [LuaReturn],
) {
    if returns.len() < 2 || returns.iter().any(|r| r.name.is_some()) {
        return;
    }
    if let Some(names) = find_return_tuple_names(body)
        && names.len() == returns.len()
    {
        for (ret, name) in returns.iter_mut().zip(names) {
            ret.name = name;
        }
    }
}

/// Walk the body to find the return tuple expression and extract element names.
pub(in crate::extract) fn find_return_tuple_names<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<Option<String>>> {
    match &expr.kind {
        // Ok((a, b, c)) — unwrap the Ok() call
        hir::ExprKind::Call(callee, args) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                let name = match qpath {
                    hir::QPath::Resolved(_, path) => {
                        path.segments.last().map(|s| s.ident.name.as_str())
                    }
                    hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str()),
                };
                if matches!(name, Some("Ok" | "Some")) && args.len() == 1 {
                    return find_return_tuple_names(&args[0]);
                }
            }
            None
        }
        // (a, b, c) tuple literal
        hir::ExprKind::Tup(elements) => {
            Some(elements.iter().map(|e| extract_return_name(e)).collect())
        }
        // Block — check tail expression
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr {
                return find_return_tuple_names(tail);
            }
            None
        }
        // Match (try desugar `?`) — look through
        hir::ExprKind::Match(scrut, _, hir::MatchSource::TryDesugar(_)) => {
            find_return_tuple_names(scrut)
        }
        // Regular match — check first arm
        hir::ExprKind::Match(_, arms, _) => {
            for arm in *arms {
                if let Some(names) = find_return_tuple_names(arm.body) {
                    return Some(names);
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract a name from a tuple element expression for return value naming.
/// Handles field accesses (`self.x` → "x"), variable references (`pos` → "pos"),
/// and method calls (`self.get_x()` → "x").
pub(in crate::extract) fn extract_return_name(expr: &hir::Expr<'_>) -> Option<String> {
    match &expr.kind {
        // self.field or obj.field → "field"
        hir::ExprKind::Field(_, ident) => {
            let name = ident.name.as_str();
            // Skip underscore-prefixed names
            if name.starts_with('_') {
                None
            } else {
                Some(name.to_string())
            }
        }
        // Variable reference → use variable name
        hir::ExprKind::Path(hir::QPath::Resolved(_, path)) => {
            if let Some(seg) = path.segments.last() {
                let name = seg.ident.name.as_str();
                if name.starts_with('_')
                    || name == "self"
                    || name.chars().next().is_some_and(|c| c.is_uppercase())
                {
                    None
                } else {
                    Some(name.to_string())
                }
            } else {
                None
            }
        }
        // method_call(...)? → unwrap try and recurse
        hir::ExprKind::Match(scrut, _, hir::MatchSource::TryDesugar(_)) => {
            extract_return_name(scrut)
        }
        // method().into_lua() or similar chains → look at receiver
        hir::ExprKind::MethodCall(seg, receiver, _, _) => {
            let method = seg.ident.name.as_str();
            // Skip conversion methods, look through to receiver
            if matches!(
                method,
                "into_lua"
                    | "into_lua_multi"
                    | "clone"
                    | "to_owned"
                    | "to_string"
                    | "into"
                    | "as_ref"
                    | "borrow"
            ) {
                return extract_return_name(receiver);
            }
            // get_x() → "x", x() → "x"
            let name = method.strip_prefix("get_").unwrap_or(method);
            if name.starts_with('_') {
                None
            } else {
                Some(name.to_string())
            }
        }
        // Function call: look at callee for associated function name
        hir::ExprKind::Call(callee, _) => {
            if let hir::ExprKind::Path(qpath) = &callee.kind {
                match qpath {
                    hir::QPath::TypeRelative(_, seg) => {
                        let name = seg.ident.name.as_str();
                        if name == "new"
                            || name.starts_with('_')
                            || name.chars().next().is_some_and(|c| c.is_uppercase())
                        {
                            None
                        } else {
                            Some(name.to_string())
                        }
                    }
                    hir::QPath::Resolved(_, path) => path.segments.last().and_then(|s| {
                        let name = s.ident.name.as_str();
                        (!name.chars().next().is_some_and(|c| c.is_uppercase()))
                            .then(|| name.to_string())
                    }),
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract names from a tuple expression's elements.
pub(in crate::extract) fn extract_tuple_element_names<'tcx>(
    receiver: &'tcx hir::Expr<'tcx>,
) -> Vec<Option<String>> {
    match &receiver.kind {
        hir::ExprKind::Tup(elements) => elements.iter().map(|e| extract_return_name(e)).collect(),
        // Look through blocks to find the tuple
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr {
                return extract_tuple_element_names(tail);
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Search an expression tree for `.into_lua_multi()` calls on tuple receivers.
/// Returns the decomposed tuple elements as multiple Lua return values with names.
pub(in crate::extract) fn find_into_lua_multi_tuple<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<Vec<LuaReturn>> {
    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, _, _) => {
            if segment.ident.name.as_str() == "into_lua_multi" {
                let typeck = tcx.typeck(receiver.hir_id.owner.def_id);
                let receiver_ty = typeck.expr_ty(receiver);
                if let ty::TyKind::Tuple(fields) = receiver_ty.kind()
                    && !fields.is_empty()
                {
                    let names = extract_tuple_element_names(receiver);
                    let returns: Vec<LuaReturn> = fields
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let ty = map_ty_to_lua(tcx, t);
                            let name = names.get(i).and_then(|n| n.clone());
                            LuaReturn { ty, name }
                        })
                        .collect();
                    return Some(returns);
                }
            }
            // Recurse
            if let Some(r) = find_into_lua_multi_tuple(tcx, receiver) {
                return Some(r);
            }
            None
        }
        hir::ExprKind::Block(block, _) => {
            if let Some(tail) = block.expr
                && let Some(r) = find_into_lua_multi_tuple(tcx, tail)
            {
                return Some(r);
            }
            for stmt in block.stmts {
                if let hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) = &stmt.kind
                    && let Some(r) = find_into_lua_multi_tuple(tcx, e)
                {
                    return Some(r);
                }
            }
            None
        }
        hir::ExprKind::Match(scrut, arms, _) => {
            if let Some(r) = find_into_lua_multi_tuple(tcx, scrut) {
                return Some(r);
            }
            for arm in *arms {
                if let Some(r) = find_into_lua_multi_tuple(tcx, arm.body) {
                    return Some(r);
                }
            }
            None
        }
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            find_into_lua_multi_tuple(tcx, body.value)
        }
        hir::ExprKind::Call(callee, args) => {
            if let Some(r) = find_into_lua_multi_tuple(tcx, callee) {
                return Some(r);
            }
            for arg in *args {
                if let Some(r) = find_into_lua_multi_tuple(tcx, arg) {
                    return Some(r);
                }
            }
            None
        }
        hir::ExprKind::If(_, then_expr, else_expr) => {
            if let Some(r) = find_into_lua_multi_tuple(tcx, then_expr) {
                return Some(r);
            }
            if let Some(e) = else_expr {
                find_into_lua_multi_tuple(tcx, e)
            } else {
                None
            }
        }
        _ => None,
    }
}
