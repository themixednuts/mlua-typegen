use mlua_typegen::{LuaType, make_union};
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use super::super::{path_tail_name, peel_expr};
use super::{
    infer_any_userdata_param_type_from_body, infer_sequence_values_item_type,
    infer_userdata_binding_type_from_body, qpath_last_name,
};

pub(in crate::extract) fn string_bytes_binding_name(expr: &hir::Expr<'_>) -> Option<String> {
    let expr = peel_expr(expr);
    let hir::ExprKind::MethodCall(segment, receiver, _, _) = &expr.kind else {
        return None;
    };
    if segment.ident.name.as_str() != "as_bytes" {
        return None;
    }
    path_tail_name(receiver)
}

pub(in crate::extract) fn tuple_scrutinee_param_positions(
    expr: &hir::Expr<'_>,
    param_name: &str,
) -> Vec<usize> {
    let expr = peel_expr(expr);
    let hir::ExprKind::Tup(items) = &expr.kind else {
        return Vec::new();
    };

    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let name = path_tail_name(item).or_else(|| string_bytes_binding_name(item));
            (name.as_deref() == Some(param_name)).then_some(index)
        })
        .collect()
}

pub(in crate::extract) fn tuple_arm_param_type(
    tcx: TyCtxt<'_>,
    pat: &hir::Pat<'_>,
    positions: &[usize],
) -> Option<LuaType> {
    let hir::PatKind::Tuple(pats, _) = &pat.kind else {
        return None;
    };

    let mut tys = Vec::new();
    for position in positions {
        let pat = pats.get(*position)?;
        if let Some(literal) = string_literal_from_pat(tcx, pat) {
            tys.push(LuaType::StringLiteral(vec![literal]));
        }
    }

    if tys.is_empty() {
        None
    } else {
        Some(make_union(tys))
    }
}

pub(in crate::extract) fn string_literal_from_pat(
    tcx: TyCtxt<'_>,
    pat: &hir::Pat<'_>,
) -> Option<String> {
    let snippet = tcx.sess.source_map().span_to_snippet(pat.span).ok()?;
    let snippet = snippet.trim();

    if let Some(value) = snippet
        .strip_prefix("b\"")
        .and_then(|s| s.strip_suffix('"'))
    {
        Some(value.to_string())
    } else {
        snippet
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .map(|s| s.to_string())
    }
}

pub(in crate::extract) fn lua_type_from_value_arm<'tcx>(
    tcx: TyCtxt<'tcx>,
    arm: &'tcx hir::Arm<'tcx>,
) -> Option<LuaType> {
    match &arm.pat.kind {
        hir::PatKind::Or(pats) => {
            let tys: Option<Vec<_>> = pats
                .iter()
                .map(|pat| lua_type_from_value_pat_with_body(tcx, pat, arm.body))
                .collect();
            tys.map(make_union)
        }
        _ => lua_type_from_value_pat_with_body(tcx, arm.pat, arm.body),
    }
}

pub(in crate::extract) fn lua_type_from_value_pat_with_body<'tcx>(
    tcx: TyCtxt<'tcx>,
    pat: &'tcx hir::Pat<'tcx>,
    body: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    match &pat.kind {
        hir::PatKind::TupleStruct(qpath, _, _)
        | hir::PatKind::Struct(qpath, _, _)
        | hir::PatKind::Expr(hir::PatExpr {
            kind: hir::PatExprKind::Path(qpath),
            ..
        }) => match qpath_last_name(qpath)?.as_str() {
            "Nil" => Some(LuaType::Nil),
            "Boolean" => Some(LuaType::Boolean),
            "Integer" => Some(LuaType::Integer),
            "Number" => Some(LuaType::Number),
            "String" => Some(LuaType::String),
            "Table" => extract_first_binding_name(pat)
                .and_then(|binding| infer_sequence_values_item_type(tcx, body, &binding))
                .map(|ty| LuaType::Array(Box::new(ty)))
                .or(Some(LuaType::Table)),
            "UserData" => extract_first_binding_name(pat)
                .and_then(|binding| infer_userdata_binding_type_from_body(tcx, body, &binding))
                .or_else(|| {
                    extract_first_binding_name(pat).and_then(|binding| {
                        infer_any_userdata_param_type_from_body(tcx, body, &binding)
                    })
                }),
            "Function" => Some(LuaType::Function),
            "Thread" => Some(LuaType::Thread),
            _ => None,
        },
        _ => None,
    }
}

pub(in crate::extract) fn extract_first_binding_name(pat: &hir::Pat<'_>) -> Option<String> {
    match &pat.kind {
        hir::PatKind::Binding(_, _, ident, _) => Some(ident.name.as_str().to_string()),
        hir::PatKind::TupleStruct(_, pats, _) | hir::PatKind::Tuple(pats, _) => {
            pats.iter().find_map(|pat| extract_first_binding_name(pat))
        }
        _ => None,
    }
}
