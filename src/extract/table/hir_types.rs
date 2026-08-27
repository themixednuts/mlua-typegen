use mlua_typegen::{LuaType, typemap::map_rust_type};
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use super::super::{
    def_snippet, extract_type_name_from_qpath, is_better_inferred_type, is_informative,
    lua_type_from_extracted_name, map_ty_to_lua, peel_expr,
};

pub(in crate::extract) fn lua_type_from_hir_ty<'hir, A>(ty: &hir::Ty<'hir, A>) -> Option<LuaType> {
    match &ty.kind {
        hir::TyKind::Ref(_, inner) => lua_type_from_hir_ty(inner.ty),
        hir::TyKind::Path(qpath) => {
            let (name, args) = match qpath {
                hir::QPath::Resolved(_, path) => {
                    let seg = path.segments.last()?;
                    (seg.ident.name.as_str(), seg.args)
                }
                hir::QPath::TypeRelative(_, seg) => (seg.ident.name.as_str(), seg.args),
            };

            let first_type_arg = || {
                args.and_then(|args| match args.args.first() {
                    Some(hir::GenericArg::Type(ty)) => Some(*ty),
                    _ => None,
                })
            };

            match name {
                "Option" => Some(LuaType::Optional(Box::new(lua_type_from_hir_ty(
                    first_type_arg()?,
                )?))),
                "Vec" | "VecDeque" => Some(LuaType::Array(Box::new(lua_type_from_hir_ty(
                    first_type_arg()?,
                )?))),
                "UserDataRef" | "UserDataRefMut" => lua_type_from_hir_ty(first_type_arg()?),
                "String" | "str" => Some(LuaType::String),
                "Integer" => Some(LuaType::Integer),
                "Number" => Some(LuaType::Number),
                "Boolean" => Some(LuaType::Boolean),
                "bool" => Some(LuaType::Boolean),
                "f32" | "f64" => Some(LuaType::Number),
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => Some(LuaType::Integer),
                "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => Some(LuaType::Integer),
                "Table" => Some(LuaType::Table),
                "Function" => Some(LuaType::Function),
                "Thread" => Some(LuaType::Thread),
                "Value" | "AnyUserData" => Some(LuaType::Any),
                _ => Some(LuaType::Class(extract_type_name_from_qpath(qpath)?)),
            }
        }
        _ => None,
    }
}

pub(in crate::extract) fn resolved_hir_ty_lua_type<'hir, A>(
    tcx: TyCtxt<'_>,
    ty: &hir::Ty<'hir, A>,
) -> Option<LuaType> {
    let hir::TyKind::Path(qpath) = &ty.kind else {
        return None;
    };

    let (res, def_id) = match qpath {
        hir::QPath::Resolved(_, path) => (path.res, path.res.opt_def_id()?),
        hir::QPath::TypeRelative(_, _) => return None,
    };

    // Only call type_of on definitions that actually have types (structs,
    // enums, type aliases, etc.). Modules, consts, and other non-type defs
    // will ICE if passed to type_of.
    let has_type = matches!(
        res,
        rustc_hir::def::Res::Def(
            rustc_hir::def::DefKind::Struct
                | rustc_hir::def::DefKind::Enum
                | rustc_hir::def::DefKind::Union
                | rustc_hir::def::DefKind::TyAlias
                | rustc_hir::def::DefKind::AssocTy
                | rustc_hir::def::DefKind::ForeignTy
                | rustc_hir::def::DefKind::OpaqueTy
                | rustc_hir::def::DefKind::TraitAlias,
            _
        ) | rustc_hir::def::Res::SelfTyAlias { .. }
            | rustc_hir::def::Res::SelfTyParam { .. }
    );

    let resolved = type_alias_def_lua_type(tcx, def_id).or_else(|| {
        if !has_type {
            return None;
        }
        let ty = map_ty_to_lua(tcx, tcx.type_of(def_id).skip_binder());
        is_informative(&ty).then_some(ty)
    })?;
    let fallback = lua_type_from_hir_ty(ty);

    match fallback.as_ref() {
        Some(existing) if *existing == resolved => None,
        Some(existing)
            if !is_informative(existing)
                || is_better_inferred_type(existing, &resolved)
                || matches!(existing, LuaType::Class(_)) =>
        {
            Some(resolved)
        }
        None => Some(resolved),
        _ => None,
    }
}

pub(in crate::extract) fn type_alias_def_lua_type(
    tcx: TyCtxt<'_>,
    def_id: rustc_hir::def_id::DefId,
) -> Option<LuaType> {
    let snippet = def_snippet(tcx, def_id)?;
    let eq = snippet.find('=')?;
    if !snippet[..eq].contains("type ") {
        return None;
    }

    let rhs = snippet[eq + 1..].split(';').next()?.trim();
    (!rhs.is_empty()).then(|| lua_type_from_extracted_name(rhs))
}

pub(in crate::extract) fn lua_type_from_hir_ty_with_snippet<'hir, A>(
    tcx: TyCtxt<'_>,
    ty: &hir::Ty<'hir, A>,
) -> Option<LuaType> {
    resolved_hir_ty_lua_type(tcx, ty)
        .or_else(|| ty_snippet_to_lua_type(tcx, ty))
        .or_else(|| lua_type_from_hir_ty(ty))
}

pub(in crate::extract) fn ty_snippet_to_lua_type<'hir, A>(
    tcx: TyCtxt<'_>,
    ty: &hir::Ty<'hir, A>,
) -> Option<LuaType> {
    // Try to resolve via HIR type structure first
    if let Some(lua_ty) = hir_ty_to_lua_type(tcx, ty)
        && is_informative(&lua_ty)
    {
        return Some(lua_ty);
    }
    // Fallback to snippet parsing
    let mut snippet = tcx.sess.source_map().span_to_snippet(ty.span).ok()?;
    snippet = snippet.trim().to_string();

    while let Some(rest) = snippet.strip_prefix('&') {
        snippet = rest.trim_start().to_string();
    }
    if let Some(rest) = snippet.strip_prefix("mut ") {
        snippet = rest.trim_start().to_string();
    }

    (!snippet.is_empty()).then(|| lua_type_from_extracted_name(&snippet))
}

/// Unwrap `Result<T, _>` from an HIR return type, returning the inner T as LuaType.
pub(in crate::extract) fn unwrap_hir_result_type<A>(
    tcx: TyCtxt<'_>,
    ty: &hir::Ty<'_, A>,
) -> Option<LuaType> {
    match &ty.kind {
        hir::TyKind::Path(qpath) => {
            let last_seg = match qpath {
                hir::QPath::Resolved(_, path) => path.segments.last()?,
                hir::QPath::TypeRelative(_, seg) => seg,
            };
            let name = last_seg.ident.name.as_str();
            if name == "Result" {
                // Result<T, _> — get the first type arg
                if let Some(args) = last_seg.args
                    && let Some(hir::GenericArg::Type(inner_ty)) = args.args.first()
                {
                    return hir_ty_to_lua_type(tcx, inner_ty);
                }
                // Result<()> or bare Result
                return Some(LuaType::Nil);
            }
            // Not a Result — convert directly
            hir_ty_to_lua_type(tcx, ty)
        }
        _ => hir_ty_to_lua_type(tcx, ty),
    }
}

/// Convert an HIR type to LuaType using the type structure instead of snippets.
#[allow(clippy::only_used_in_recursion)]
pub(in crate::extract) fn hir_ty_to_lua_type<A>(
    tcx: TyCtxt<'_>,
    ty: &hir::Ty<'_, A>,
) -> Option<LuaType> {
    match &ty.kind {
        hir::TyKind::Ref(_, ref_ty) => hir_ty_to_lua_type(tcx, ref_ty.ty),
        hir::TyKind::Path(qpath) => {
            let path_str = match qpath {
                hir::QPath::Resolved(_, path) => path
                    .segments
                    .iter()
                    .map(|s| s.ident.name.as_str())
                    .collect::<Vec<_>>()
                    .join("::"),
                hir::QPath::TypeRelative(_, seg) => seg.ident.name.as_str().to_string(),
            };

            // Extract generic args if present
            let type_args: Vec<LuaType> = match qpath {
                hir::QPath::Resolved(_, path) => path
                    .segments
                    .last()
                    .and_then(|seg| seg.args)
                    .map(|args| {
                        args.args
                            .iter()
                            .filter_map(|arg| {
                                if let hir::GenericArg::Type(inner_ty) = arg {
                                    hir_ty_to_lua_type(tcx, inner_ty)
                                } else {
                                    None
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                _ => vec![],
            };

            let mapped = map_rust_type(&path_str, &type_args);
            Some(mapped)
        }
        hir::TyKind::Tup([]) => Some(LuaType::Nil),
        _ => None,
    }
}

pub(in crate::extract) fn method_generic_lua_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
    segment: &hir::PathSegment<'_>,
) -> Option<LuaType> {
    // Try explicit generic args from HIR first
    if let Some(args) = segment.args
        && let Some(hir::GenericArg::Type(ty)) = args.args.first()
        && let Some(lua_ty) = lua_type_from_hir_ty_with_snippet(tcx, ty)
    {
        return Some(lua_ty);
    }

    // Try typeck-resolved generic args (handles inferred generics)
    typeck_method_generic_type(tcx, expr)
}

/// Resolve the first generic type argument of a method call via typeck.
/// This handles cases where the turbofish is elided and the compiler infers the type.
pub(in crate::extract) fn typeck_method_generic_type(
    tcx: TyCtxt<'_>,
    expr: &hir::Expr<'_>,
) -> Option<LuaType> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let args = typeck.node_args(expr.hir_id);
    // Method calls have Self as first arg, then user-specified type params
    // Skip Self and receiver-related args, look for the first type that maps informatively
    for arg in args.iter() {
        if let Some(ty) = arg.as_type() {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if is_informative(&lua_ty)
                && !matches!(&lua_ty, LuaType::Class(n) if n.contains("{opaque"))
            {
                return Some(lua_ty);
            }
        }
    }
    None
}

pub(in crate::extract) fn callee_try_from_type_name(
    _tcx: TyCtxt<'_>,
    callee: &hir::Expr<'_>,
) -> Option<String> {
    let callee = peel_expr(callee);
    match &callee.kind {
        hir::ExprKind::Path(qpath) => match qpath {
            hir::QPath::TypeRelative(ty, seg) if seg.ident.name.as_str() == "try_from" => {
                let hir::TyKind::Path(qpath) = &ty.kind else {
                    return None;
                };
                extract_type_name_from_qpath(qpath)
            }
            hir::QPath::Resolved(_, path) => {
                let seg = path.segments.last()?;
                if seg.ident.name.as_str() != "try_from" {
                    return None;
                }
                let prev = path.segments.iter().rev().nth(1)?;
                Some(prev.ident.name.as_str().to_string())
            }
            _ => None,
        },
        _ => None,
    }
}

pub(in crate::extract) fn qpath_last_name(qpath: &hir::QPath<'_>) -> Option<String> {
    match qpath {
        hir::QPath::Resolved(_, path) => path
            .segments
            .last()
            .map(|seg| seg.ident.name.as_str().to_string()),
        hir::QPath::TypeRelative(_, seg) => Some(seg.ident.name.as_str().to_string()),
    }
}
