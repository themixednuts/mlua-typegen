use mlua_typegen::{ExtractedApi, LuaClass};
use rustc_hir as hir;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_middle::ty::{self, TyCtxt};

use super::{extract_type_doc, replace_self_class_alias, replace_self_return_sentinel};

#[derive(Clone)]
pub(super) struct RegistrationOwner {
    def_id: DefId,
    name: String,
}

impl RegistrationOwner {
    pub(super) fn for_impl(tcx: TyCtxt<'_>, impl_def_id: LocalDefId) -> Option<Self> {
        owner_from_type(tcx, tcx.type_of(impl_def_id).skip_binder())
    }

    pub(super) fn empty_class(&self, tcx: TyCtxt<'_>) -> LuaClass {
        LuaClass {
            name: self.name.clone(),
            doc: extract_type_doc(tcx, tcx.type_of(self.def_id).skip_binder()),
            fields: Vec::new(),
            methods: Vec::new(),
        }
    }

    pub(super) fn rust_path(&self, tcx: TyCtxt<'_>) -> String {
        super::qualified_def_path_str(tcx, self.def_id)
    }

    pub(super) fn lua_name(&self) -> &str {
        &self.name
    }

    pub(super) fn finish_class(&self, mut class: LuaClass) -> LuaClass {
        for field in &mut class.fields {
            replace_self_return_sentinel(&mut field.ty, &self.name);
            replace_self_class_alias(&mut field.ty, &self.name);
        }
        for method in &mut class.methods {
            for parameter in &mut method.params {
                replace_self_class_alias(&mut parameter.ty, &self.name);
            }
            for value in &mut method.returns {
                replace_self_return_sentinel(&mut value.ty, &self.name);
                replace_self_class_alias(&mut value.ty, &self.name);
            }
        }
        class
    }
}

pub(super) fn registration_owner_for_body(
    tcx: TyCtxt<'_>,
    body_id: hir::BodyId,
) -> Option<RegistrationOwner> {
    let body = tcx.hir_body(body_id);
    let parameter = body.params.first()?;
    let typeck = tcx.typeck(parameter.hir_id.owner.def_id);
    registry_owner_from_type(tcx, typeck.node_type(parameter.pat.hir_id))
}

fn registry_owner_from_type(tcx: TyCtxt<'_>, value: ty::Ty<'_>) -> Option<RegistrationOwner> {
    let value = match value.kind() {
        ty::TyKind::Ref(_, inner, _) | ty::TyKind::RawPtr(inner, _) => *inner,
        _ => value,
    };
    let ty::TyKind::Adt(registry, arguments) = value.kind() else {
        return None;
    };
    let registry_id = registry.did();
    if tcx.crate_name(registry_id.krate).as_str() != "mlua"
        || tcx.item_name(registry_id).as_str() != "UserDataRegistry"
    {
        return None;
    }
    owner_from_type(tcx, arguments.types().next()?)
}

fn owner_from_type(tcx: TyCtxt<'_>, owner: ty::Ty<'_>) -> Option<RegistrationOwner> {
    let ty::TyKind::Adt(owner, _) = owner.kind() else {
        return None;
    };
    Some(RegistrationOwner {
        def_id: owner.did(),
        name: tcx.item_name(owner.did()).as_str().to_owned(),
    })
}

pub(super) fn upsert_class(api: &mut ExtractedApi, mut incoming: LuaClass) {
    if let Some(existing) = api
        .classes
        .iter_mut()
        .find(|class| class.name == incoming.name)
    {
        if existing.doc.is_none() {
            existing.doc = incoming.doc.take();
        }
        existing.fields.append(&mut incoming.fields);
        existing.methods.append(&mut incoming.methods);
    } else {
        api.classes.push(incoming);
    }
}
