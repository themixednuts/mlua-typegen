use mlua_typegen::{LuaParam, LuaReturn, LuaType, typemap::map_rust_type};
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use super::qualified_def_path_str;

pub(super) fn extract_params_from_tuple<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: ty::Ty<'tcx>,
    hir_names: &[String],
) -> Vec<LuaParam> {
    match ty.kind() {
        ty::TyKind::Tuple(fields) => fields
            .iter()
            .enumerate()
            .map(|(i, field_ty)| LuaParam {
                name: hir_names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("p{}", i + 1)),
                ty: map_ty_to_lua(tcx, field_ty),
            })
            .collect(),
        _ => {
            if ty.is_unit() {
                Vec::new()
            } else {
                let name = hir_names
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "p1".to_string());
                vec![LuaParam {
                    name,
                    ty: map_ty_to_lua(tcx, ty),
                }]
            }
        }
    }
}

/// Check if a trait path is a Fn-family trait (Fn, FnMut, FnOnce, async variants).
pub(super) fn is_fn_trait(path: &str) -> bool {
    matches!(
        path,
        "std::ops::Fn"
            | "core::ops::Fn"
            | "std::ops::FnMut"
            | "core::ops::FnMut"
            | "std::ops::FnOnce"
            | "core::ops::FnOnce"
            | "std::ops::AsyncFn"
            | "core::ops::AsyncFn"
            | "std::ops::AsyncFnMut"
            | "core::ops::AsyncFnMut"
            | "std::ops::AsyncFnOnce"
            | "core::ops::AsyncFnOnce"
    )
}

/// Find a projection for a given associated type name (e.g. "Item", "Target", "Output").
pub(super) fn find_projection<'tcx>(
    tcx: TyCtxt<'tcx>,
    predicates: &'tcx ty::List<ty::PolyExistentialPredicate<'tcx>>,
    assoc_name: &str,
) -> Option<ty::Ty<'tcx>> {
    for pred in predicates.iter() {
        if let ty::ExistentialPredicate::Projection(proj) = pred.skip_binder()
            && tcx.item_name(proj.def_id).as_str() == assoc_name
        {
            return proj.term.as_type();
        }
    }
    None
}

/// Map a `dyn Trait` type by inspecting the principal trait and projections.
pub(super) fn map_dyn_trait<'tcx>(
    tcx: TyCtxt<'tcx>,
    predicates: &'tcx ty::List<ty::PolyExistentialPredicate<'tcx>>,
) -> LuaType {
    for pred in predicates.iter() {
        if let ty::ExistentialPredicate::Trait(trait_ref) = pred.skip_binder() {
            let trait_path = tcx.def_path_str(trait_ref.def_id);

            // dyn Fn(A, B) -> C → fun(p1: A, p2: B): C
            if is_fn_trait(&trait_path) {
                // Fn trait args are encoded as the trait's generic args (tuple of params)
                let params: Vec<LuaType> = trait_ref
                    .args
                    .iter()
                    .filter_map(|arg| arg.as_type())
                    // The first arg is the tuple of parameters
                    .next()
                    .map(|tuple_ty| match tuple_ty.kind() {
                        ty::TyKind::Tuple(fields) => {
                            fields.iter().map(|t| map_ty_to_lua(tcx, t)).collect()
                        }
                        _ => vec![map_ty_to_lua(tcx, tuple_ty)],
                    })
                    .unwrap_or_default();

                // Return type is in the Output projection
                let returns = find_projection(tcx, predicates, "Output")
                    .map(|ret_ty| {
                        let lua_ty = map_ty_to_lua(tcx, ret_ty);
                        if lua_ty == LuaType::Nil {
                            vec![]
                        } else {
                            vec![lua_ty]
                        }
                    })
                    .unwrap_or_default();

                if params.is_empty() && returns.is_empty() {
                    return LuaType::Function;
                }
                return LuaType::FunctionSig { params, returns };
            }
        }
    }
    LuaType::UnsupportedStatic {
        rust_type: "dynamic trait object".to_owned(),
        reason: "no native mlua conversion contract matches the resolved principal trait"
            .to_owned(),
    }
}

/// Convert a rustc `ty::Ty` to a `LuaType`.
pub(super) fn map_ty_to_lua<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> LuaType {
    match ty.kind() {
        ty::TyKind::Bool => LuaType::Boolean,
        ty::TyKind::Int(_) | ty::TyKind::Uint(_) => LuaType::Integer,
        ty::TyKind::Float(_) => LuaType::Number,
        ty::TyKind::Str => LuaType::String,
        ty::TyKind::Char => LuaType::String,
        ty::TyKind::Tuple(fields) if fields.is_empty() => LuaType::Nil,
        ty::TyKind::Never => LuaType::Nil,

        ty::TyKind::Ref(_, inner, _) => map_ty_to_lua(tcx, *inner),
        ty::TyKind::RawPtr(inner, _) => map_ty_to_lua(tcx, *inner),

        ty::TyKind::Slice(inner) => LuaType::Array(Box::new(map_ty_to_lua(tcx, *inner))),

        ty::TyKind::Array(inner, _len) => LuaType::Array(Box::new(map_ty_to_lua(tcx, *inner))),

        // Function pointers → extract typed signature
        ty::TyKind::FnPtr(sig_tys, _hdr) => {
            let sig = sig_tys.skip_binder();
            let params: Vec<LuaType> = sig
                .inputs()
                .iter()
                .map(|t| map_ty_to_lua(tcx, *t))
                .collect();
            let ret = map_ty_to_lua(tcx, sig.output());
            let returns = if ret == LuaType::Nil {
                vec![]
            } else {
                vec![ret]
            };
            if params.is_empty() && returns.is_empty() {
                LuaType::Function
            } else {
                LuaType::FunctionSig { params, returns }
            }
        }

        // FnDef and closures → function (signature not easily extractable inline)
        ty::TyKind::FnDef(..) | ty::TyKind::Closure(..) => LuaType::Function,

        // dyn Trait → inspect the trait to pick a better type
        ty::TyKind::Dynamic(predicates, ..) => map_dyn_trait(tcx, predicates),

        ty::TyKind::Adt(adt_def, args) => {
            let path = qualified_def_path_str(tcx, adt_def.did());

            let type_args: Vec<LuaType> = args.types().map(|t| map_ty_to_lua(tcx, t)).collect();
            let mapped = map_rust_type(&path, &type_args);
            if matches!(mapped, LuaType::UnresolvedRust { .. }) && !adt_def.did().is_local() {
                LuaType::ExternalRust {
                    path,
                    arguments: type_args,
                    stable_crate_id: tcx.stable_crate_id(adt_def.did().krate).as_u64(),
                }
            } else {
                mapped
            }
        }

        #[cfg(rustc_alias_is_rigid)]
        ty::TyKind::Alias(_, alias) => map_alias_ty_to_lua(tcx, alias),

        #[cfg(all(rustc_alias_kind_field, not(rustc_alias_is_rigid)))]
        ty::TyKind::Alias(alias) => map_alias_ty_to_lua(tcx, alias),

        #[cfg(not(rustc_alias_kind_field))]
        ty::TyKind::Alias(_, alias) => map_alias_ty_to_lua(tcx, alias),

        _ => LuaType::UnsupportedStatic {
            rust_type: ty.to_string(),
            reason: "this rustc type form has no implemented exact lowering rule".to_owned(),
        },
    }
}

pub(super) fn map_alias_ty_to_lua<'tcx>(tcx: TyCtxt<'tcx>, alias: &ty::AliasTy<'tcx>) -> LuaType {
    map_ty_to_lua(tcx, instantiate_alias_ty(tcx, alias))
}

#[cfg(rustc_alias_kind_struct_variants)]
pub(super) fn alias_def_id(alias: &ty::AliasTy<'_>) -> rustc_hir::def_id::DefId {
    match alias.kind {
        ty::AliasTyKind::Projection { def_id } => def_id,
        ty::AliasTyKind::Inherent { def_id } => def_id,
        ty::AliasTyKind::Opaque { def_id } => def_id,
        ty::AliasTyKind::Free { def_id } => def_id,
    }
}

#[cfg(all(rustc_alias_kind_field, not(rustc_alias_kind_struct_variants)))]
pub(super) fn alias_def_id(alias: &ty::AliasTy<'_>) -> rustc_hir::def_id::DefId {
    alias.kind.def_id()
}

#[cfg(not(rustc_alias_kind_field))]
pub(super) fn alias_def_id(alias: &ty::AliasTy<'_>) -> rustc_hir::def_id::DefId {
    alias.def_id
}

#[cfg(rustc_alias_kind_struct_variants)]
pub(super) fn alias_term_def_id(term: ty::AliasTerm<'_>) -> rustc_hir::def_id::DefId {
    match term.kind {
        ty::AliasTermKind::ProjectionTy { def_id } => def_id,
        ty::AliasTermKind::InherentTy { def_id } => def_id,
        ty::AliasTermKind::OpaqueTy { def_id } => def_id,
        ty::AliasTermKind::FreeTy { def_id } => def_id,
        ty::AliasTermKind::AnonConst { def_id } => def_id,
        ty::AliasTermKind::ProjectionConst { def_id } => def_id,
        ty::AliasTermKind::FreeConst { def_id } => def_id,
        ty::AliasTermKind::InherentConst { def_id } => def_id,
    }
}

#[cfg(all(rustc_alias_kind_field, not(rustc_alias_kind_struct_variants)))]
pub(super) fn alias_term_def_id(term: ty::AliasTerm<'_>) -> rustc_hir::def_id::DefId {
    term.def_id()
}

#[cfg(not(rustc_alias_kind_field))]
pub(super) fn alias_term_def_id(term: ty::AliasTerm<'_>) -> rustc_hir::def_id::DefId {
    term.def_id
}

#[cfg(rustc_instantiate_unnormalized)]
pub(super) fn instantiate_alias_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    alias: &ty::AliasTy<'tcx>,
) -> ty::Ty<'tcx> {
    tcx.type_of(alias_def_id(alias))
        .instantiate(tcx, alias.args)
        .skip_norm_wip()
}

#[cfg(not(rustc_instantiate_unnormalized))]
pub(super) fn instantiate_alias_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    alias: &ty::AliasTy<'tcx>,
) -> ty::Ty<'tcx> {
    tcx.type_of(alias_def_id(alias))
        .instantiate(tcx, alias.args)
}

#[cfg(all(rustc_instantiate_unnormalized, rustc_early_binder_bind_tcx))]
pub(super) fn instantiate_bound_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: ty::Ty<'tcx>,
    args: ty::GenericArgsRef<'tcx>,
) -> ty::Ty<'tcx> {
    ty::EarlyBinder::bind(tcx, ty)
        .instantiate(tcx, args)
        .skip_norm_wip()
}

#[cfg(all(rustc_instantiate_unnormalized, not(rustc_early_binder_bind_tcx)))]
pub(super) fn instantiate_bound_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: ty::Ty<'tcx>,
    args: ty::GenericArgsRef<'tcx>,
) -> ty::Ty<'tcx> {
    ty::EarlyBinder::bind(ty)
        .instantiate(tcx, args)
        .skip_norm_wip()
}

#[cfg(all(not(rustc_instantiate_unnormalized), not(rustc_early_binder_bind_tcx)))]
pub(super) fn instantiate_bound_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: ty::Ty<'tcx>,
    args: ty::GenericArgsRef<'tcx>,
) -> ty::Ty<'tcx> {
    ty::EarlyBinder::bind(ty).instantiate(tcx, args)
}

#[cfg(rustc_alias_is_rigid)]
pub(super) fn alias_from_ty<'tcx>(ty: ty::Ty<'tcx>) -> Option<&'tcx ty::AliasTy<'tcx>> {
    match ty.kind() {
        ty::TyKind::Alias(_, alias) => Some(alias),
        _ => None,
    }
}

#[cfg(all(rustc_alias_kind_field, not(rustc_alias_is_rigid)))]
pub(super) fn alias_from_ty<'tcx>(ty: ty::Ty<'tcx>) -> Option<&'tcx ty::AliasTy<'tcx>> {
    match ty.kind() {
        ty::TyKind::Alias(alias) => Some(alias),
        _ => None,
    }
}

#[cfg(not(rustc_alias_kind_field))]
pub(super) fn alias_from_ty<'tcx>(ty: ty::Ty<'tcx>) -> Option<&'tcx ty::AliasTy<'tcx>> {
    match ty.kind() {
        ty::TyKind::Alias(_, alias) => Some(alias),
        _ => None,
    }
}

/// Returns true if the type is an error class that leaked through Result unwrapping.
/// In Lua, errors are thrown (not returned), so these should be stripped from returns.
pub(super) fn is_error_class(ty: &LuaType) -> bool {
    matches!(ty, LuaType::Class(name) if name == "Error" || name == "LuaError"
        || name.ends_with("::Error") || name.ends_with("::LuaError"))
}

/// Map a return type to a list of Lua return values.
/// Handles Result unwrapping and tuple decomposition for multiple returns.
pub(super) fn map_return_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> Vec<LuaReturn> {
    // Unwrap Coroutine (async closures) to get the actual return type
    let ty = unwrap_coroutine_ty(tcx, ty);
    // Unwrap impl Future<Output = T> from async fn signatures
    let ty = unwrap_future_output(tcx, ty);
    // Then unwrap Result<T, _> → T
    let ty = unwrap_result_ty(tcx, ty);

    match ty.kind() {
        // Empty tuple = no return values
        ty::TyKind::Tuple(fields) if fields.is_empty() => Vec::new(),

        // Non-empty tuple = multiple return values
        ty::TyKind::Tuple(fields) => fields
            .iter()
            .map(|t| LuaReturn::from(map_ty_to_lua(tcx, t)))
            .filter(|r| !is_error_class(&r.ty))
            .collect(),

        // Single return value
        _ => {
            let lua_ty = map_ty_to_lua(tcx, ty);
            if lua_ty == LuaType::Nil || is_error_class(&lua_ty) {
                Vec::new()
            } else {
                vec![lua_ty.into()]
            }
        }
    }
}

/// Unwrap Result<T, _> to get T. If not a Result, returns the type unchanged.
/// Unwrap `impl Future<Output = T>` from async fn signatures.
/// The opaque type from `fn_sig` resolves through its bounds to find the Output projection.
pub(super) fn unwrap_future_output<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> ty::Ty<'tcx> {
    let Some(alias) = alias_from_ty(ty) else {
        return ty;
    };

    let alias_def_id = alias_def_id(alias);
    if !matches!(
        tcx.def_kind(alias_def_id),
        rustc_hir::def::DefKind::OpaqueTy
    ) {
        return ty;
    }

    let bounds = tcx.explicit_item_bounds(alias_def_id);
    for (bound, _) in bounds.skip_binder() {
        if let ty::ClauseKind::Projection(proj) = bound.kind().skip_binder()
            && tcx
                .item_name(alias_term_def_id(proj.projection_term))
                .as_str()
                == "Output"
            && let Some(output_ty) = proj.term.as_type()
        {
            return instantiate_bound_ty(tcx, output_ty, alias.args);
        }
    }
    ty
}

pub(super) fn unwrap_result_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> ty::Ty<'tcx> {
    match ty.kind() {
        ty::TyKind::Adt(adt_def, args) => {
            let path = tcx.def_path_str(adt_def.did());
            if (path == "std::result::Result" || path == "core::result::Result")
                && let Some(inner) = args.types().next()
            {
                return inner;
            }
            ty
        }
        // Type aliases like anyhow::Result<T> resolve through Alias
        #[cfg(rustc_alias_is_rigid)]
        ty::TyKind::Alias(_, alias) => {
            let instantiated = instantiate_alias_ty(tcx, alias);
            let unwrapped = unwrap_result_ty(tcx, instantiated);
            if unwrapped != instantiated {
                unwrapped
            } else {
                ty
            }
        }
        #[cfg(all(rustc_alias_kind_field, not(rustc_alias_is_rigid)))]
        ty::TyKind::Alias(alias) => {
            let instantiated = instantiate_alias_ty(tcx, alias);
            let unwrapped = unwrap_result_ty(tcx, instantiated);
            if unwrapped != instantiated {
                unwrapped
            } else {
                ty
            }
        }
        #[cfg(not(rustc_alias_kind_field))]
        ty::TyKind::Alias(_, alias) => {
            let instantiated = instantiate_alias_ty(tcx, alias);
            let unwrapped = unwrap_result_ty(tcx, instantiated);
            if unwrapped != instantiated {
                unwrapped
            } else {
                ty
            }
        }
        _ => ty,
    }
}

/// Unwrap a Coroutine type (from async closures) to find the Result<T, E> return type.
/// Async closures produce: Coroutine(DefId, [move_id, (), ResumeTy, (), Result<T, E>, ...])
pub(super) fn unwrap_coroutine_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> ty::Ty<'tcx> {
    if let ty::TyKind::Coroutine(_, args) = ty.kind() {
        let mut fallback = None;

        // Coroutine args contain many intermediate future/output types.
        // Prefer the last Result<_, mlua::Error>, which best matches the
        // async closure's actual output, and otherwise fall back to the last Result.
        for arg in args.iter().rev() {
            let Some(inner_ty) = arg.as_type() else {
                continue;
            };
            let ty::TyKind::Adt(adt_def, result_args) = inner_ty.kind() else {
                continue;
            };

            let path = tcx.def_path_str(adt_def.did());
            if path != "std::result::Result" && path != "core::result::Result" {
                continue;
            }

            if fallback.is_none() {
                fallback = Some(inner_ty);
            }

            if let Some(err_ty) = result_args.types().nth(1)
                && let ty::TyKind::Adt(err_def, _) = err_ty.kind()
            {
                let err_path = tcx.def_path_str(err_def.did());
                if (err_path.contains("mlua") && err_path.ends_with("Error"))
                    || err_path.ends_with("mlua::Error")
                    || err_path == "mlua::Error"
                {
                    return inner_ty;
                }
            }
        }

        if let Some(result_ty) = fallback {
            return result_ty;
        }
    }
    ty
}

/// Infer the Lua type of a value expression using typeck.
pub(super) fn infer_expr_lua_type(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> LuaType {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    let ty = typeck.expr_ty(expr);
    map_ty_to_lua(tcx, ty)
}
