use std::cell::RefCell;

use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::{self, TyCtxt};

use mlua_typegen::{ExtractedUnit, LuaField, LuaType, config::MappingId};

mod class;
mod conversion;
mod diagnostic;
mod emit;
mod fields;
mod finish;
mod hir_util;
mod lower;
mod lua_type;
mod mapping_validation;
mod param;
mod pipeline;
mod plan;
mod returns;
mod signature;
mod source;
mod table;
mod trace;
mod userdata_registration;
mod walk;

use class::{enclosing_impl_self_type_name, record_method_registration, type_display_name};
use emit::{
    infer_value_expr_lua_type, infer_wrapper_method_lua_type, is_passthrough_method,
    merge_body_inference, merge_inferred_returns, rewrap_erased_body_inference,
    should_prefer_body_inference, should_replace_returns,
};
use fields::{find_into_lua_receiver_type, infer_from_expr, record_field_registration};
use hir_util::{
    expr_def_id, extract_string_literal, find_cross_module_call_in_hir, find_local_binding_init,
    infer_local_table_binding_type, is_try_desugar_expr, local_binding_info, owner_body,
    path_tail_name, peel_expr, peel_lua_conversion_expr, peel_to_dynamic, peel_try_expr,
    resolve_table_get_function_name, unwrap_try_expr,
};
use lower::{
    extract_params_from_tuple, infer_expr_lua_type, instantiate_alias_ty, map_return_ty,
    map_ty_to_lua, unwrap_result_ty,
};

use lua_type::{
    contains_self_return_sentinel, is_better_inferred_type, is_informative,
    lua_type_from_extracted_name, normalize_explicit_param_type, normalize_inferred_lua_type,
    normalize_param_lua_type, normalize_table_value_types, replace_self_class_alias,
    replace_self_return_sentinel, self_return_sentinel, should_prefer_explicit_param_type,
};
use param::infer_param_type_from_body;
use returns::{
    enrich_return_names, extract_type_from_closure_expr, extract_type_name_from_qpath,
    find_into_lua_multi_tuple, infer_concrete_type_from_body, infer_forwarded_multivalue_returns,
    infer_multi_returns_from_body, resolve_any_user_data_class, try_lua_value_constructor,
};
use signature::{
    extract_closure_signature, extract_meta_method_name, infer_field_closure_result,
    infer_local_callable_param_type, infer_local_fn_body_return, infer_mapped_method_result,
};
use source::{
    def_snippet, expr_snippet, extract_doc_comments, extract_type_doc, qualified_def_path_str,
};
use table::{
    body_returns_named_userdata_self, callee_try_from_type_name, expr_refers_to_binding,
    infer_any_userdata_param_type_from_body, infer_callback_signature_from_body,
    infer_from_lua_conversion_type, infer_from_value_binding_type,
    infer_json_like_param_type_from_body, infer_param_type_from_converter_call,
    infer_param_type_from_local_call, infer_param_type_from_local_method_call,
    infer_table_param_type_from_body, infer_table_param_type_from_converter_calls,
    infer_take_binding_type, infer_userdata_binding_type_from_body, is_any_userdata_type_snippet,
    is_method_registration_name, lua_type_from_value_arm, pat_to_name, resolve_local_method_def_id,
    string_bytes_binding_name, string_literal_from_pat, tuple_arm_param_type,
    tuple_scrutinee_param_positions,
};
use trace::{
    should_trace_field_expr_snippet, should_trace_field_name, should_trace_method_name, trace,
    trace_class_snapshot,
};
use userdata_registration::{RegistrationOwner, upsert_class};
use walk::{find_in_recursive_expr_children, visit_recursive_expr_children};

thread_local! {
    static LOCAL_PARAM_INFERENCE_STACK: RefCell<Vec<(LocalDefId, usize)>> = const { RefCell::new(Vec::new()) };
}

struct LocalParamInferenceGuard(LocalDefId, usize);

impl Drop for LocalParamInferenceGuard {
    fn drop(&mut self) {
        LOCAL_PARAM_INFERENCE_STACK.with(|stack| {
            let popped = stack.borrow_mut().pop();
            debug_assert_eq!(popped, Some((self.0, self.1)));
        });
    }
}

fn enter_local_param_inference(
    def_id: LocalDefId,
    param_index: usize,
) -> Option<LocalParamInferenceGuard> {
    let already_in_progress = LOCAL_PARAM_INFERENCE_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.contains(&(def_id, param_index)) {
            true
        } else {
            stack.push((def_id, param_index));
            false
        }
    });

    (!already_in_progress).then_some(LocalParamInferenceGuard(def_id, param_index))
}

/// Check whether a type is or contains `mlua::AnyUserData`.
/// Matches: `AnyUserData`, `Option<AnyUserData>`, and `Result<AnyUserData, _>`.
fn is_any_user_data(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> bool {
    if let ty::TyKind::Adt(adt_def, substs) = ty.kind() {
        let path = tcx.def_path_str(adt_def.did());
        if path.ends_with("AnyUserData") {
            return true;
        }
        // Check Option<AnyUserData> or Result<AnyUserData, _>
        if (path.ends_with("Option") || path.ends_with("Result"))
            && !substs.is_empty()
            && let Some(inner) = substs[0].as_type()
        {
            return is_any_user_data(tcx, inner);
        }
    }
    false
}

/// Extract and assemble the full Lua API surface from the crate.
pub(super) fn extract_unit(
    tcx: TyCtxt<'_>,
    config: &mlua_typegen::config::ResolvedConfig,
    evidence: &mlua_typegen::driver_protocol::EvidenceCatalog,
    scope: Option<&std::collections::BTreeSet<String>>,
) -> Result<CompiledUnit, diagnostic::CompileDiagnostics> {
    pipeline::ApiCompiler::new(tcx, config, evidence, scope).compile()
}

pub(super) struct CompiledUnit {
    pub unit: ExtractedUnit,
    pub used_mappings: std::collections::BTreeSet<MappingId>,
    pub requirements: std::collections::BTreeSet<mlua_typegen::driver_protocol::RequiredEvidence>,
    pub provided_classes: std::collections::BTreeMap<String, String>,
}

pub(crate) use diagnostic::CompileDiagnostics;

fn writable_field(name: String, ty: LuaType) -> LuaField {
    LuaField {
        name,
        ty,
        writable: true,
        doc: None,
    }
}

fn readonly_field(name: String, ty: LuaType) -> LuaField {
    LuaField {
        name,
        ty,
        writable: false,
        doc: None,
    }
}

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;

    for (idx, ch) in text.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            _ => {}
        }

        if ch == separator && angle == 0 && paren == 0 && bracket == 0 {
            parts.push(text[start..idx].trim());
            start = idx + ch.len_utf8();
        }
    }

    if start == 0 {
        return vec![text.trim()];
    }

    parts.push(text[start..].trim());
    parts
}
