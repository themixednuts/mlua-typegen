use rustc_hir as hir;
use rustc_hir::attrs::AttributeKind;
use rustc_middle::ty::{self, TyCtxt};

pub(super) fn expr_snippet(tcx: TyCtxt<'_>, expr: &hir::Expr<'_>) -> String {
    tcx.sess
        .source_map()
        .span_to_snippet(expr.span)
        .unwrap_or_else(|_| format!("<{:?}>", expr.kind))
}

pub(super) fn qualified_def_path_str(tcx: TyCtxt<'_>, def_id: rustc_hir::def_id::DefId) -> String {
    let path = tcx.def_path_str(def_id);
    if !def_id.is_local() {
        return path;
    }

    let crate_name = tcx
        .crate_name(rustc_hir::def_id::LOCAL_CRATE)
        .as_str()
        .to_string();
    if path == crate_name || path.starts_with(&format!("{crate_name}::")) {
        path
    } else {
        format!("{crate_name}::{path}")
    }
}

pub(super) fn def_snippet(tcx: TyCtxt<'_>, def_id: rustc_hir::def_id::DefId) -> Option<String> {
    let sm = tcx.sess.source_map();
    let span = def_id.as_local().map_or_else(
        || tcx.def_span(def_id),
        |local| match tcx.hir_node_by_def_id(local) {
            rustc_hir::Node::Item(item) => item.span,
            rustc_hir::Node::ImplItem(item) => item.span,
            rustc_hir::Node::TraitItem(item) => item.span,
            rustc_hir::Node::Expr(expr) => expr.span,
            _ => tcx.def_span(def_id),
        },
    );

    sm.span_to_snippet(span)
        .ok()
        .filter(|snippet| snippet.contains('\n') || snippet.len() > 64)
        .or_else(|| def_file_context(tcx, span))
}

fn def_file_context(tcx: TyCtxt<'_>, span: rustc_span::Span) -> Option<String> {
    let sm = tcx.sess.source_map();
    let filename = local_filename(sm.span_to_filename(span));
    if filename.starts_with('<') {
        return None;
    }

    let start = sm.lookup_char_pos(span.lo()).line.saturating_sub(1);
    let end = sm.lookup_char_pos(span.hi()).line.saturating_add(80);
    let text = std::fs::read_to_string(filename).ok()?;
    let lines: Vec<_> = text.lines().collect();
    if start >= lines.len() {
        return None;
    }

    Some(lines[start..end.min(lines.len())].join("\n"))
}

#[cfg(rustc_filename_prefer_local)]
fn local_filename(filename: rustc_span::FileName) -> String {
    filename.prefer_local_unconditionally().to_string()
}

#[cfg(not(rustc_filename_prefer_local))]
fn local_filename(filename: rustc_span::FileName) -> String {
    filename.prefer_remapped_unconditionally().to_string()
}

/// Extract doc comments (`#[doc = "..."]`) from HIR attributes for a given HirId.
pub(super) fn extract_doc_comments(tcx: TyCtxt<'_>, hir_id: hir::HirId) -> Option<String> {
    let lines: Vec<_> = tcx
        .hir_attrs(hir_id)
        .iter()
        .filter_map(extract_doc_from_attr)
        .map(|comment| comment.strip_prefix(' ').unwrap_or(comment))
        .collect();

    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn extract_doc_from_attr(attr: &hir::Attribute) -> Option<&str> {
    match attr {
        hir::Attribute::Parsed(AttributeKind::DocComment { comment, .. }) => Some(comment.as_str()),
        _ => None,
    }
}

/// Extract doc comments for a type definition (struct/enum) by DefId.
pub(super) fn extract_type_doc(tcx: TyCtxt<'_>, ty: ty::Ty<'_>) -> Option<String> {
    if let ty::TyKind::Adt(adt_def, _) = ty.kind() {
        let def_id = adt_def.did();
        if let Some(local_id) = def_id.as_local() {
            let hir_id = tcx.local_def_id_to_hir_id(local_id);
            return extract_doc_comments(tcx, hir_id);
        }
    }
    None
}
