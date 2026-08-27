mod any_userdata;
mod callbacks;
mod converters;
mod hir_types;
mod param_tables;
mod patterns;
mod terminals;
mod userdata;

pub(super) use any_userdata::{
    infer_any_userdata_param_type_from_body, infer_from_lua_conversion_type,
    infer_json_like_param_type_from_body,
};
pub(super) use callbacks::{
    infer_callback_signature_from_body, infer_from_value_binding_type, infer_take_binding_type,
};
pub(super) use converters::{
    called_path_name, extract_type_id_guard_lua_type, infer_param_type_from_converter_call,
    infer_param_type_from_local_call, infer_param_type_from_local_method_call,
    infer_value_converter_types_from_hir, infer_value_converter_types_from_snippet,
    is_type_id_call_on_binding, json_like_lua_type, path_contains_segment,
    resolve_local_method_def_id,
};
pub(super) use hir_types::{
    callee_try_from_type_name, hir_ty_to_lua_type, lua_type_from_hir_ty,
    lua_type_from_hir_ty_with_snippet, method_generic_lua_type, qpath_last_name,
    typeck_method_generic_type, unwrap_hir_result_type,
};
pub(super) use param_tables::{
    infer_table_param_type_from_body, infer_table_param_type_from_converter_calls,
    is_any_userdata_type_snippet,
};
pub(super) use patterns::{
    lua_type_from_value_arm, string_bytes_binding_name, string_literal_from_pat,
    tuple_arm_param_type, tuple_scrutinee_param_positions,
};
pub(super) use terminals::{
    body_returns_named_userdata_self, is_method_registration_name, pat_to_name,
    terminal_return_is_opaque_any_userdata,
};
pub(super) use userdata::{
    expr_refers_to_binding, infer_sequence_values_item_type, infer_userdata_binding_type_from_body,
};
