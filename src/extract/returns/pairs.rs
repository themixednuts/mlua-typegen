use mlua_typegen::{LuaReturn, LuaType};
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use super::super::{
    instantiate_alias_ty, lua_type_from_extracted_name, map_ty_to_lua, peel_expr, peel_try_expr,
    qualified_def_path_str, split_top_level,
};

pub(in crate::extract) fn infer_pairs_forward_returns(state: &LuaType) -> Vec<LuaReturn> {
    let value = infer_pairs_value_type(state).unwrap_or_else(|| LuaType::UnsupportedStatic {
        rust_type: state.to_string(),
        reason: "the iterator state does not expose an exact pair value type".to_owned(),
    });
    vec![
        LuaType::FunctionSig {
            params: vec![state.clone()],
            returns: vec![
                LuaType::Optional(Box::new(LuaType::Integer)),
                LuaType::Optional(Box::new(value)),
            ],
        }
        .into(),
        state.clone().into(),
        LuaType::Nil.into(),
    ]
}

pub(in crate::extract) fn infer_pairs_state_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    state: &LuaType,
) -> LuaType {
    let LuaType::Class(name) = state else {
        return state.clone();
    };

    // Handle bare "Iter" or "Iter<...>" (where the first generic arg is an
    // internal Rust iterator type that shouldn't leak into Lua annotations).
    let is_iter = name == "Iter" || name.starts_with("Iter<");
    if !is_iter {
        return state.clone();
    }

    // If the class name already carries a value type (second generic arg),
    // extract it directly rather than re-inferring from the expression.
    if let Some(value) = extract_iter_value_arg_from_class_name(name) {
        return LuaType::Class(format!("Iter<any, {value}>"));
    }

    // Bare "Iter" — try to recover the value type from the expression.
    let Some(value) = infer_iter_alias_value_type(tcx, expr)
        .or_else(|| infer_iter_alias_value_type_from_wrapper_arg(tcx, expr))
    else {
        return state.clone();
    };

    LuaType::Class(format!("Iter<any, {value}>"))
}

/// Given a class name like `"Iter<SomeComplexType, Url>"`, extract the second
/// generic argument (`"Url"`) if present.
pub(in crate::extract) fn extract_iter_value_arg_from_class_name(name: &str) -> Option<String> {
    let inner = name.strip_prefix("Iter<")?.strip_suffix('>')?;
    // Split at top-level commas (respecting nested angle brackets).
    let mut depth = 0i32;
    let mut last_comma = None;
    for (i, ch) in inner.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => last_comma = Some(i),
            _ => {}
        }
    }
    let comma_pos = last_comma?;
    let value = inner[comma_pos + 1..].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub(in crate::extract) fn infer_iter_alias_value_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let ty = tcx.typeck(expr.hir_id.owner.def_id).expr_ty(expr);
    extract_iter_value_type_from_ty(tcx, ty)
}

pub(in crate::extract) fn infer_iter_alias_value_type_from_wrapper_arg<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_expr(peel_try_expr(expr));
    let hir::ExprKind::MethodCall(_, _, args, _) = &expr.kind else {
        return None;
    };

    args.get(1).and_then(|arg| {
        let arg = peel_expr(peel_try_expr(arg));
        if let hir::ExprKind::MethodCall(segment, receiver, _, _) = &arg.kind
            && segment.ident.name.as_str() == "clone"
        {
            infer_iter_alias_value_type(tcx, receiver)
        } else {
            infer_iter_alias_value_type(tcx, arg)
        }
    })
}

pub(in crate::extract) fn extract_iter_value_type_from_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: ty::Ty<'tcx>,
) -> Option<LuaType> {
    match ty.kind() {
        #[cfg(rustc_alias_is_rigid)]
        ty::TyKind::Alias(_, alias) => {
            let instantiated = instantiate_alias_ty(tcx, alias);
            extract_iter_value_type_from_ty(tcx, instantiated)
        }
        #[cfg(all(rustc_alias_kind_field, not(rustc_alias_is_rigid)))]
        ty::TyKind::Alias(alias) => {
            let instantiated = instantiate_alias_ty(tcx, alias);
            extract_iter_value_type_from_ty(tcx, instantiated)
        }
        #[cfg(not(rustc_alias_kind_field))]
        ty::TyKind::Alias(_, alias) => {
            let instantiated = instantiate_alias_ty(tcx, alias);
            extract_iter_value_type_from_ty(tcx, instantiated)
        }
        ty::TyKind::Adt(adt_def, args) => {
            let path = qualified_def_path_str(tcx, adt_def.did());
            if path.ends_with("::Iter") || path == "Iter" {
                args.types().nth(1).map(|ty| map_ty_to_lua(tcx, ty))
            } else {
                None
            }
        }
        ty::TyKind::Ref(_, inner, _) => extract_iter_value_type_from_ty(tcx, *inner),
        _ => None,
    }
}

pub(in crate::extract) fn infer_pairs_value_type(state: &LuaType) -> Option<LuaType> {
    let LuaType::Class(name) = state else {
        return None;
    };
    if name == "Iter" {
        return Some(LuaType::Any);
    }
    if let Some((base, args)) = parse_embedded_class_args(name) {
        return (base == "Iter" && args.len() >= 2).then(|| args[1].clone());
    }

    None
}

pub(in crate::extract) fn parse_embedded_class_args(name: &str) -> Option<(&str, Vec<LuaType>)> {
    let start = name.find('<')?;
    let inner = name.get(start + 1..name.len().checked_sub(1)?)?;
    name.ends_with('>').then_some(())?;

    Some((
        name[..start].trim(),
        split_top_level(inner, ',')
            .into_iter()
            .map(lua_type_from_extracted_name)
            .collect(),
    ))
}
