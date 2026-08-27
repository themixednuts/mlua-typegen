mod events;
mod function_value;
mod module_export;
mod registration;
mod table_module;
mod value_inference;

pub(super) use events::extract_string_literal_from_expr;
pub(super) use registration::collect_body;
pub(super) use value_inference::{
    infer_value_expr_lua_type, infer_wrapper_method_lua_type, is_passthrough_method,
    merge_body_inference, merge_inferred_returns, rewrap_erased_body_inference,
    should_prefer_body_inference, should_replace_returns,
};
