use std::collections::{BTreeSet, HashMap};

use rustc_hir as hir;
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_middle::ty::TyCtxt;

use super::conversion::{ConversionDirection, ConversionSubject};
use super::userdata_registration::RegistrationOwner;

pub(super) struct ExtractionPlan<'tcx> {
    work: Vec<ExtractionWork<'tcx>>,
    free_bodies: HashMap<LocalDefId, usize>,
    generated_modules: HashMap<LocalDefId, String>,
}

pub(super) enum ExtractionWork<'tcx> {
    Class {
        owner: RegistrationOwner,
    },
    Conversion(ConversionSubject),
    Body {
        body_id: hir::BodyId,
        module_item: Option<&'tcx hir::Item<'tcx>>,
        generated_module_name: Option<String>,
        registration_owner: Option<RegistrationOwner>,
    },
}

impl<'tcx> ExtractionPlan<'tcx> {
    pub(super) fn for_crate(tcx: TyCtxt<'tcx>, scope: Option<&BTreeSet<String>>) -> Self {
        let mut plan = Self {
            work: Vec::new(),
            free_bodies: HashMap::new(),
            generated_modules: HashMap::new(),
        };

        for item_id in tcx.hir_crate_items(()).free_items() {
            let item = tcx.hir_item(item_id);
            plan.collect_item(tcx, item, scope);
        }

        plan
    }

    pub(super) fn into_work(self) -> Vec<ExtractionWork<'tcx>> {
        self.work
    }

    fn collect_item(
        &mut self,
        tcx: TyCtxt<'tcx>,
        item: &'tcx hir::Item<'tcx>,
        scope: Option<&BTreeSet<String>>,
    ) {
        match &item.kind {
            hir::ItemKind::Impl(impl_block) => self.collect_impl(tcx, item, impl_block, scope),
            hir::ItemKind::Fn { body, .. } => {
                let registration_owner =
                    super::userdata_registration::registration_owner_for_body(tcx, *body);
                if let Some(scope) = scope
                    && !registration_owner
                        .as_ref()
                        .is_some_and(|owner| scope.contains(&owner.rust_path(tcx)))
                {
                    return;
                }
                if let Some((source, module_name)) = generated_module_source(tcx, item) {
                    self.record_generated_module(source, module_name);
                    return;
                }
                let owner = item.owner_id.def_id;
                let index = self.work.len();
                self.work.push(ExtractionWork::Body {
                    body_id: *body,
                    module_item: Some(item),
                    generated_module_name: self.generated_modules.remove(&owner),
                    registration_owner,
                });
                self.free_bodies.insert(owner, index);
            }
            _ => {}
        }
    }

    fn collect_impl(
        &mut self,
        tcx: TyCtxt<'tcx>,
        item: &'tcx hir::Item<'tcx>,
        impl_block: &'tcx hir::Impl<'tcx>,
        scope: Option<&BTreeSet<String>>,
    ) {
        let mut conversion = None;
        let registration_owner = impl_trait_def_id(impl_block).and_then(|trait_def_id| {
            if is_mlua_trait(tcx, trait_def_id, "UserData") {
                RegistrationOwner::for_impl(tcx, item.owner_id.def_id)
            } else if let Some(direction) = conversion_direction(tcx, trait_def_id)
                && let Some(subject) =
                    ConversionSubject::from_impl(tcx, item.owner_id.def_id, direction)
            {
                conversion = Some(subject);
                None
            } else {
                None
            }
        });
        if let Some(scope) = scope {
            if let Some(owner) = registration_owner
                .clone()
                .filter(|owner| scope.contains(&owner.rust_path(tcx)))
            {
                self.work.push(ExtractionWork::Class {
                    owner: owner.clone(),
                });
                self.collect_registration_bodies(tcx, impl_block, owner);
            } else if let Some(subject) = conversion
                && scope.contains(&subject.rust_path(tcx))
            {
                self.work.push(ExtractionWork::Conversion(subject));
            }
            return;
        }

        if let Some(subject) = conversion {
            self.work.push(ExtractionWork::Conversion(subject));
        }
        if let Some(owner) = registration_owner.clone() {
            self.work.push(ExtractionWork::Class { owner });
        }

        for impl_item_ref in impl_block.items {
            let impl_item_id = hir::ImplItemId {
                owner_id: impl_item_ref.owner_id,
            };
            let impl_item = tcx.hir_impl_item(impl_item_id);
            if let hir::ImplItemKind::Fn(_, body_id) = impl_item.kind {
                let registration_owner = registration_owner.clone().filter(|_| {
                    matches!(
                        impl_item.ident.name.as_str(),
                        "add_fields" | "add_methods" | "register"
                    )
                });
                self.work.push(ExtractionWork::Body {
                    body_id,
                    module_item: None,
                    generated_module_name: None,
                    registration_owner,
                });
            }
        }
    }

    fn collect_registration_bodies(
        &mut self,
        tcx: TyCtxt<'tcx>,
        impl_block: &'tcx hir::Impl<'tcx>,
        owner: RegistrationOwner,
    ) {
        for impl_item_ref in impl_block.items {
            let impl_item = tcx.hir_impl_item(hir::ImplItemId {
                owner_id: impl_item_ref.owner_id,
            });
            if let hir::ImplItemKind::Fn(_, body_id) = impl_item.kind
                && matches!(
                    impl_item.ident.name.as_str(),
                    "add_fields" | "add_methods" | "register"
                )
            {
                self.work.push(ExtractionWork::Body {
                    body_id,
                    module_item: None,
                    generated_module_name: None,
                    registration_owner: Some(owner.clone()),
                });
            }
        }
    }

    fn record_generated_module(&mut self, source: LocalDefId, module_name: String) {
        if let Some(index) = self.free_bodies.get(&source).copied()
            && let ExtractionWork::Body {
                generated_module_name,
                ..
            } = &mut self.work[index]
        {
            *generated_module_name = Some(module_name);
        } else {
            self.generated_modules.insert(source, module_name);
        }
    }
}

fn generated_module_source(tcx: TyCtxt<'_>, item: &hir::Item<'_>) -> Option<(LocalDefId, String)> {
    let hir::ItemKind::Fn { body, .. } = item.kind else {
        return None;
    };
    let module_name = item
        .kind
        .ident()?
        .name
        .as_str()
        .strip_prefix("luaopen_")?
        .to_owned();
    let mut facts = ModuleWrapperFacts::default();
    collect_module_wrapper_facts(tcx, tcx.hir_body(body).value, &mut facts);
    facts
        .has_mlua_entrypoint
        .then_some(facts.source?)
        .map(|source| (source, module_name))
}

#[derive(Default)]
struct ModuleWrapperFacts {
    has_mlua_entrypoint: bool,
    source: Option<LocalDefId>,
}

fn collect_module_wrapper_facts<'tcx>(
    tcx: TyCtxt<'tcx>,
    expr: &'tcx hir::Expr<'tcx>,
    facts: &mut ModuleWrapperFacts,
) {
    if let Some(def_id) = super::expr_def_id(tcx, expr).and_then(DefId::as_local)
        && matches!(tcx.hir_node_by_def_id(def_id), hir::Node::Item(_))
        && matches!(tcx.def_kind(def_id), rustc_hir::def::DefKind::Fn)
    {
        facts.source.get_or_insert(def_id);
    }

    let resolved = match expr.kind {
        hir::ExprKind::Call(callee, _) => super::expr_def_id(tcx, callee),
        hir::ExprKind::MethodCall(..) => tcx
            .typeck(expr.hir_id.owner.def_id)
            .type_dependent_def_id(expr.hir_id),
        _ => None,
    };
    facts.has_mlua_entrypoint |= resolved.is_some_and(|def_id| {
        tcx.crate_name(def_id.krate).as_str() == "mlua"
            && tcx.item_name(def_id).as_str() == "entrypoint1"
    });

    if let hir::ExprKind::Closure(closure) = expr.kind {
        collect_module_wrapper_facts(tcx, tcx.hir_body(closure.body).value, facts);
    }

    super::visit_recursive_expr_children(expr, |child| {
        collect_module_wrapper_facts(tcx, child, facts);
    });
}

fn impl_trait_def_id(impl_block: &hir::Impl<'_>) -> Option<DefId> {
    impl_block
        .of_trait
        .as_ref()?
        .trait_ref
        .path
        .res
        .opt_def_id()
}

fn is_mlua_trait(tcx: TyCtxt<'_>, trait_def_id: DefId, name: &str) -> bool {
    tcx.crate_name(trait_def_id.krate).as_str() == "mlua"
        && tcx.item_name(trait_def_id).as_str() == name
}

fn conversion_direction(tcx: TyCtxt<'_>, trait_def_id: DefId) -> Option<ConversionDirection> {
    if is_mlua_trait(tcx, trait_def_id, "IntoLua") {
        Some(ConversionDirection::IntoLua)
    } else if is_mlua_trait(tcx, trait_def_id, "FromLua") {
        Some(ConversionDirection::FromLua)
    } else {
        None
    }
}
