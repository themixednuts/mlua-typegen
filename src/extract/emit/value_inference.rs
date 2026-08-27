use mlua_typegen::{ExtractedApi, LuaFunction, LuaModule, LuaReturn, LuaType, make_union};
use rustc_hir as hir;
use rustc_middle::ty::{self, TyCtxt};

use crate::extract::{
    emit::function_value::extract_standalone_closure_signature,
    hir_util::{extract_string_literal, owner_body, peel_lua_conversion_expr, peel_try_expr},
    lower::{infer_expr_lua_type, map_ty_to_lua},
    lua_type::{is_better_inferred_type, is_informative, lua_runtime_type},
    returns::{
        extract_type_from_closure_expr, infer_concrete_type_from_body, try_lua_value_constructor,
    },
    table::method_generic_lua_type,
    writable_field,
};

use super::function_value::try_extract_create_function;
use super::registration::upsert_module;
use super::table_module::try_extract_module_from_value_expr;

pub(in crate::extract) enum ExportedValue {
    Function(LuaFunction),
    NestedModule(ExtractedModule),
    Field(LuaType),
}

pub(in crate::extract) fn classify_exported_value<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    name: &str,
    nested_module_name: Option<&str>,
) -> ExportedValue {
    if let Some(func) = try_extract_create_function(tcx, expr, name) {
        return ExportedValue::Function(func);
    }

    if let Some(module_name) = nested_module_name
        && let Some(module) = try_extract_module_from_value_expr(tcx, expr, module_name)
    {
        return ExportedValue::NestedModule(module);
    }

    ExportedValue::Field(infer_value_expr_lua_type(tcx, expr))
}

pub(in crate::extract) fn push_exported_value(
    module: &mut LuaModule,
    name: String,
    value: ExportedValue,
    nested_modules: Option<&mut Vec<LuaModule>>,
) {
    match value {
        ExportedValue::Function(func) => module.functions.push(func),
        ExportedValue::NestedModule(extracted) => {
            let nested_name = extracted.module.name.clone();
            module
                .fields
                .push(writable_field(name, LuaType::Class(nested_name)));
            if let Some(nested_modules) = nested_modules {
                nested_modules.extend(extracted.nested_modules);
                nested_modules.push(extracted.module);
            }
        }
        ExportedValue::Field(ty) => module.fields.push(writable_field(name, ty)),
    }
}

pub(in crate::extract) struct ExtractedModule {
    pub(in crate::extract) module: LuaModule,
    pub(in crate::extract) nested_modules: Vec<LuaModule>,
}

pub(in crate::extract) fn upsert_extracted_module(
    api: &mut ExtractedApi,
    extracted: ExtractedModule,
) -> String {
    let root_name = extracted.module.name.clone();
    for module in extracted.nested_modules {
        upsert_module(api, module);
    }
    upsert_module(api, extracted.module);
    root_name
}

pub(in crate::extract) fn infer_value_expr_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> LuaType {
    let expr = peel_lua_conversion_expr(expr);

    if let Some(ty) = infer_wrapper_method_lua_type(tcx, expr)
        && (is_informative(&ty) || matches!(ty, LuaType::Class(_) | LuaType::Optional(_)))
    {
        return ty;
    }

    if let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind {
        match segment.ident.name.as_str() {
            "create_sequence_from" => {
                if let Some(values_expr) = args.first()
                    && let Some(item_ty) = infer_sequence_item_lua_type(tcx, values_expr)
                {
                    return LuaType::Array(Box::new(item_ty));
                }
            }
            "to_value" | "to_value_with" => {
                if let Some(value_expr) = args.first() {
                    return infer_value_expr_lua_type(tcx, value_expr);
                }
            }
            _ => {}
        }
    }

    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    if let Some(ty) = try_lua_value_constructor(tcx, typeck, expr) {
        return ty;
    }

    let ty = infer_expr_lua_type(tcx, expr);
    if is_informative(&ty) {
        ty
    } else {
        infer_concrete_type_from_body(tcx, expr).unwrap_or(ty)
    }
}

pub(in crate::extract) fn infer_sequence_item_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::Array(items) => items
            .first()
            .map(|item| infer_value_expr_lua_type(tcx, item)),
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            match segment.ident.name.as_str() {
                "map" => args
                    .first()
                    .and_then(|mapper| {
                        extract_type_from_closure_expr(tcx, mapper).map(LuaType::Class)
                    })
                    .or_else(|| infer_sequence_item_lua_type(tcx, receiver)),
                "iter" | "into_iter" | "copied" | "cloned" => {
                    sequence_item_type_from_expr(tcx, receiver)
                        .or_else(|| infer_sequence_item_lua_type(tcx, receiver))
                }
                "get_args" => Some(LuaType::String),
                _ => sequence_item_type_from_expr(tcx, expr),
            }
        }
        _ => sequence_item_type_from_expr(tcx, expr),
    }
}

pub(in crate::extract) fn sequence_item_type_from_expr<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let typeck = tcx.typeck(expr.hir_id.owner.def_id);
    sequence_item_type_from_ty(tcx, typeck.expr_ty(expr))
}

pub(in crate::extract) fn sequence_item_type_from_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: ty::Ty<'tcx>,
) -> Option<LuaType> {
    match ty.kind() {
        ty::TyKind::Ref(_, inner, _) => sequence_item_type_from_ty(tcx, *inner),
        ty::TyKind::Slice(inner) | ty::TyKind::Array(inner, _) => Some(map_ty_to_lua(tcx, *inner)),
        ty::TyKind::Adt(adt, args) => {
            let path = tcx.def_path_str(adt.did());
            let name = path.rsplit("::").next().unwrap_or_default();
            match name {
                "Vec" | "VecDeque" | "LinkedList" | "HashSet" | "BTreeSet" | "IndexSet" => {
                    args.types().next().map(|inner| map_ty_to_lua(tcx, inner))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

pub(in crate::extract) fn infer_wrapper_method_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind else {
        return None;
    };

    match segment.ident.name.as_str() {
        "create_userdata" | "create_any_userdata" | "create_any_userdata_ref" => args
            .first()
            .map(|value_expr| infer_expr_lua_type(tcx, value_expr)),
        "named_registry_value" => infer_named_registry_value_lua_type(tcx, expr),
        "create_function" | "create_async_function" => args
            .first()
            .and_then(|closure| infer_created_function_signature(tcx, closure))
            .or(Some(LuaType::Function)),
        "create_table" => Some(LuaType::Table),
        "create_table_from" => args
            .first()
            .and_then(|entries| infer_table_from_entries_lua_type(tcx, entries)),
        "create_sequence_from" => args
            .first()
            .and_then(|values_expr| infer_sequence_item_lua_type(tcx, values_expr))
            .map(|item_ty| LuaType::Array(Box::new(item_ty))),
        "to_value" | "to_value_with" => Some(lua_runtime_type()),
        _ => None,
    }
}

pub(in crate::extract) fn infer_named_registry_value_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let hir::ExprKind::MethodCall(segment, _receiver, args, _) = &expr.kind else {
        return None;
    };
    let key = extract_string_literal(args.first()?)?;
    let generic_ty = method_generic_lua_type(tcx, expr, segment)?;

    if !matches!(generic_ty, LuaType::Any) {
        return Some(generic_ty);
    }

    let body = owner_body(tcx, expr.hir_id.owner.def_id)?;
    let mut value_types = collect_named_registry_value_types(tcx, body.value, &key);
    if value_types.is_empty() {
        Some(LuaType::Any)
    } else {
        Some(make_union(std::mem::take(&mut value_types)))
    }
}

pub(in crate::extract) fn collect_named_registry_value_types<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    key: &str,
) -> Vec<LuaType> {
    let expr = peel_try_expr(expr);

    match &expr.kind {
        hir::ExprKind::MethodCall(segment, receiver, args, _) => {
            let mut tys = Vec::new();
            if segment.ident.name.as_str() == "set_named_registry_value"
                && args.len() >= 2
                && extract_string_literal(&args[0]).as_deref() == Some(key)
            {
                tys.push(infer_value_expr_lua_type(tcx, &args[1]));
            }

            tys.extend(collect_named_registry_value_types(tcx, receiver, key));
            for arg in args.iter() {
                tys.extend(collect_named_registry_value_types(tcx, arg, key));
            }
            tys
        }
        hir::ExprKind::Call(callee, args) => {
            let mut tys = collect_named_registry_value_types(tcx, callee, key);
            for arg in args.iter() {
                tys.extend(collect_named_registry_value_types(tcx, arg, key));
            }
            tys
        }
        hir::ExprKind::Block(block, _) => block
            .stmts
            .iter()
            .flat_map(|stmt| match &stmt.kind {
                hir::StmtKind::Let(local) => local
                    .init
                    .map(|init| collect_named_registry_value_types(tcx, init, key))
                    .unwrap_or_default(),
                hir::StmtKind::Expr(expr) | hir::StmtKind::Semi(expr) => {
                    collect_named_registry_value_types(tcx, expr, key)
                }
                _ => Vec::new(),
            })
            .chain(
                block
                    .expr
                    .into_iter()
                    .flat_map(|expr| collect_named_registry_value_types(tcx, expr, key)),
            )
            .collect(),
        hir::ExprKind::Match(scrutinee, arms, _) => {
            let mut tys = collect_named_registry_value_types(tcx, scrutinee, key);
            for arm in arms.iter() {
                tys.extend(collect_named_registry_value_types(tcx, arm.body, key));
            }
            tys
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            let mut tys = collect_named_registry_value_types(tcx, cond, key);
            tys.extend(collect_named_registry_value_types(tcx, then_expr, key));
            if let Some(else_expr) = else_expr {
                tys.extend(collect_named_registry_value_types(tcx, else_expr, key));
            }
            tys
        }
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            let mut tys = collect_named_registry_value_types(tcx, lhs, key);
            tys.extend(collect_named_registry_value_types(tcx, rhs, key));
            tys
        }
        hir::ExprKind::Binary(_, lhs, rhs) => {
            let mut tys = collect_named_registry_value_types(tcx, lhs, key);
            tys.extend(collect_named_registry_value_types(tcx, rhs, key));
            tys
        }
        hir::ExprKind::Let(let_expr) => collect_named_registry_value_types(tcx, let_expr.init, key),
        hir::ExprKind::Array(items) | hir::ExprKind::Tup(items) => items
            .iter()
            .flat_map(|item| collect_named_registry_value_types(tcx, item, key))
            .collect(),
        hir::ExprKind::Struct(_, fields, tail) => fields
            .iter()
            .flat_map(|field| collect_named_registry_value_types(tcx, field.expr, key))
            .chain(match tail {
                hir::StructTailExpr::Base(base) => {
                    collect_named_registry_value_types(tcx, base, key)
                }
                _ => Vec::new(),
            })
            .collect(),
        hir::ExprKind::Closure(closure) => {
            let body = tcx.hir_body(closure.body);
            collect_named_registry_value_types(tcx, body.value, key)
        }
        hir::ExprKind::Ret(Some(inner)) => collect_named_registry_value_types(tcx, inner, key),
        _ => Vec::new(),
    }
}

pub(in crate::extract) fn infer_created_function_signature<'tcx>(
    tcx: TyCtxt<'tcx>,
    closure_expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let (params, returns) = extract_standalone_closure_signature(tcx, closure_expr)?;
    Some(LuaType::FunctionSig {
        params: params.into_iter().map(|param| param.ty).collect(),
        returns: returns.into_iter().map(|ret| ret.ty).collect(),
    })
}

pub(in crate::extract) fn is_passthrough_method(name: &str) -> bool {
    matches!(
        name,
        "and_then"
            | "map"
            | "map_err"
            | "ok"
            | "err"
            | "into"
            | "into_lua_err"
            | "with_context"
            | "context"
            | "clone"
            | "to_owned"
            | "borrow"
            | "as_ref"
    )
}

pub(in crate::extract) fn infer_table_from_entries_lua_type<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<LuaType> {
    let expr = peel_try_expr(expr);
    let hir::ExprKind::Array(entries) = &expr.kind else {
        return Some(LuaType::Table);
    };

    let mut value_types = Vec::new();
    for entry in entries.iter() {
        let hir::ExprKind::Tup(parts) = &entry.kind else {
            return Some(LuaType::Table);
        };
        if parts.len() != 2 || extract_string_literal(&parts[0]).is_none() {
            return Some(LuaType::Table);
        }
        value_types.push(infer_value_expr_lua_type(tcx, &parts[1]));
    }

    if value_types.is_empty() {
        return Some(LuaType::Table);
    }

    let value_ty = if value_types.len() == 1 {
        value_types
            .pop()
            .expect("the single-value branch contains one inferred value")
    } else {
        make_union(value_types)
    };

    Some(LuaType::Map(Box::new(LuaType::String), Box::new(value_ty)))
}

pub(in crate::extract) fn should_replace_returns(
    existing: &[LuaReturn],
    inferred: &[LuaReturn],
) -> bool {
    if inferred.is_empty() {
        return !existing.is_empty();
    }

    if inferred.iter().all(|ret| !is_informative(&ret.ty)) {
        return false;
    }

    if existing.is_empty() {
        return true;
    }

    if existing.len() != inferred.len() {
        return existing.iter().all(|ret| !is_informative(&ret.ty));
    }

    existing
        .iter()
        .zip(inferred)
        .all(|(existing, inferred)| should_prefer_body_inference(&existing.ty, &inferred.ty))
}

pub(in crate::extract) fn merge_inferred_returns(
    returns: &mut Vec<LuaReturn>,
    inferred: Option<Vec<LuaReturn>>,
) {
    if let Some(inferred) = inferred
        && should_replace_returns(returns, &inferred)
    {
        *returns = inferred;
    }
}

pub(in crate::extract) fn is_erased_body_type(ty: &LuaType) -> bool {
    match ty {
        LuaType::Any => true,
        LuaType::Optional(inner) => is_erased_body_type(inner),
        _ => false,
    }
}

pub(in crate::extract) fn should_prefer_body_inference(
    existing: &LuaType,
    inferred: &LuaType,
) -> bool {
    if !is_informative(inferred) {
        return false;
    }

    is_better_inferred_type(existing, inferred)
        || !is_informative(existing)
        || (is_erased_body_type(existing) && !is_erased_body_type(inferred))
}

pub(in crate::extract) fn rewrap_erased_body_inference(
    existing: &LuaType,
    inferred: &LuaType,
) -> LuaType {
    match existing {
        LuaType::Optional(inner)
            if is_erased_body_type(inner) && !matches!(inferred, LuaType::Optional(_)) =>
        {
            LuaType::Optional(Box::new(inferred.clone()))
        }
        _ => inferred.clone(),
    }
}

pub(in crate::extract) fn merge_body_inference(
    best: Option<LuaType>,
    candidate: LuaType,
) -> Option<LuaType> {
    if !is_informative(&candidate) {
        return best;
    }

    match best {
        Some(current) if should_prefer_body_inference(&current, &candidate) => {
            Some(rewrap_erased_body_inference(&current, &candidate))
        }
        Some(current) => Some(current),
        None => Some(candidate),
    }
}
