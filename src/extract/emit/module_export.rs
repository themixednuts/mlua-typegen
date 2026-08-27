use mlua_typegen::LuaModule;
use rustc_hir as hir;
use rustc_middle::ty::TyCtxt;

use crate::extract::{extract_doc_comments, extract_string_literal};

use super::value_inference::{classify_exported_value, push_exported_value};

pub(in crate::extract) fn module_for_function<'tcx>(
    tcx: TyCtxt<'tcx>,
    item: &hir::Item<'tcx>,
) -> Option<LuaModule> {
    let attrs = tcx.hir_attrs(item.hir_id());
    let mut module_attr = None;

    for attr in attrs {
        if let hir::Attribute::Unparsed(attr_item) = attr {
            let segments: Vec<_> = attr_item.path.segments.iter().map(|s| s.as_str()).collect();
            if matches!(segments.as_slice(), ["mlua", "lua_module"] | ["lua_module"]) {
                module_attr = Some(attr_item);
                break;
            }
        }
    }

    module_attr.as_ref()?;

    // Get the module name from the attribute arg or function name
    let fn_name = item
        .kind
        .ident()
        .map(|id| id.name.as_str().to_string())
        .unwrap_or_else(|| "unknown_module".to_string());

    let module_name = module_attr
        .and_then(|a| extract_name_from_attr_args(&a.args))
        .unwrap_or(fn_name);

    let doc = extract_doc_comments(tcx, item.hir_id());

    Some(LuaModule {
        name: module_name,
        doc,
        functions: Vec::new(),
        fields: Vec::new(),
    })
}

/// Try to extract `name = "value"` from attribute args.
pub(in crate::extract) fn extract_name_from_attr_args(args: &hir::AttrArgs) -> Option<String> {
    if let hir::AttrArgs::Delimited(delim) = args {
        // The tokens are stored as a TokenStream; do a simple string search
        let s = format!("{:?}", delim.tokens);
        parse_name_attr(&s)
    } else {
        None
    }
}

/// Simple parser for `name = "value"` in attribute arg strings.
pub(in crate::extract) fn parse_name_attr(s: &str) -> Option<String> {
    let idx = s.find("name")?;
    let rest = &s[idx + 4..];
    let rest = rest.trim().strip_prefix('=')?;
    let rest = rest.trim();
    // Find quoted string
    let start = rest.find('"')? + 1;
    let rest = &rest[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Walk an expression to find `table.set("name", value)` patterns for module exports.
pub(in crate::extract) fn record_module_export<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    module: &mut LuaModule,
) {
    if let Some(export_name) = extract_named_set_call(expr) {
        let NamedSetCall { name, value } = export_name;
        let exported = classify_exported_value(tcx, value, &name, None);
        push_exported_value(module, name, exported, None);
    }
}

pub(in crate::extract) struct NamedSetCall<'tcx> {
    pub(in crate::extract) name: String,
    pub(in crate::extract) value: &'tcx hir::Expr<'tcx>,
}

pub(in crate::extract) fn extract_named_set_call<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
) -> Option<NamedSetCall<'tcx>> {
    match &expr.kind {
        hir::ExprKind::MethodCall(segment, _receiver, args, _span) => {
            let method_name = segment.ident.name.as_str();
            let export_name = ((method_name == "set" || method_name == "raw_set")
                && args.len() >= 2)
                .then(|| extract_string_literal(&args[0]))
                .flatten();

            export_name.map(|name| NamedSetCall {
                name,
                value: &args[1],
            })
        }
        _ => None,
    }
}
