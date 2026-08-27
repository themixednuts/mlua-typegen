use mlua_typegen::config::{MappingId, ResolvedConfig};
use rustc_hir::def::{DefKind, Res};
use rustc_hir::def_id::{DefId, LOCAL_CRATE};
use rustc_middle::ty::{self, TyCtxt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MappingValidationError {
    AmbiguousCrate {
        mapping: MappingId,
        crate_name: String,
    },
    MissingDefinition {
        mapping: MappingId,
        rust_path: String,
    },
    NotAType {
        mapping: MappingId,
        rust_path: String,
        kind: DefKind,
        def_id: DefId,
    },
    GenericParameters {
        mapping: MappingId,
        rust_path: String,
        expected: Vec<String>,
        actual: Vec<String>,
        def_id: DefId,
    },
    NativeConflict {
        mapping: MappingId,
        rust_path: String,
        def_id: DefId,
    },
}

impl MappingValidationError {
    pub(super) fn rust_path(&self) -> &str {
        match self {
            Self::AmbiguousCrate { crate_name, .. } => crate_name,
            Self::MissingDefinition { rust_path, .. }
            | Self::NotAType { rust_path, .. }
            | Self::GenericParameters { rust_path, .. }
            | Self::NativeConflict { rust_path, .. } => rust_path,
        }
    }

    pub(super) fn def_id(&self) -> Option<DefId> {
        match self {
            Self::NotAType { def_id, .. }
            | Self::GenericParameters { def_id, .. }
            | Self::NativeConflict { def_id, .. } => Some(*def_id),
            Self::AmbiguousCrate { .. } | Self::MissingDefinition { .. } => None,
        }
    }

    pub(super) const fn remediation(&self) -> &'static str {
        match self {
            Self::AmbiguousCrate { .. } => {
                "select a Cargo package instance whose crate name is unique in this compilation"
            }
            Self::MissingDefinition { .. } => {
                "use the canonical definition path from the selected Cargo package"
            }
            Self::NotAType { .. } => "select a Rust struct, enum, union, or type alias",
            Self::GenericParameters { .. } => {
                "declare every Rust type parameter in its definition order"
            }
            Self::NativeConflict { .. } => {
                "remove the mapping because native mlua semantics have precedence"
            }
        }
    }
}

impl std::fmt::Display for MappingValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AmbiguousCrate {
                mapping,
                crate_name,
            } => write!(
                formatter,
                "mapping `{mapping}` resolves to multiple rustc crates named `{crate_name}`"
            ),
            Self::MissingDefinition { mapping, rust_path } => write!(
                formatter,
                "mapping `{mapping}` does not resolve to Rust definition `{rust_path}`"
            ),
            Self::NotAType {
                mapping,
                rust_path,
                kind,
                ..
            } => write!(
                formatter,
                "mapping `{mapping}` selects `{rust_path}`, which is a {kind:?} rather than a type"
            ),
            Self::GenericParameters {
                mapping,
                rust_path,
                expected,
                actual,
                ..
            } => write!(
                formatter,
                "mapping `{mapping}` declares generic parameters {expected:?} for `{rust_path}`, but rustc resolved {actual:?}"
            ),
            Self::NativeConflict {
                mapping, rust_path, ..
            } => write!(
                formatter,
                "mapping `{mapping}` conflicts with native semantics for `{rust_path}`"
            ),
        }
    }
}

pub(super) fn validate(tcx: TyCtxt<'_>, config: &ResolvedConfig) -> Vec<MappingValidationError> {
    let mut errors = Vec::new();
    for mapping in config.mappings() {
        let crate_name = mapping.selector().crate_name();
        let roots = crate_roots(tcx, crate_name);
        if roots.is_empty() {
            continue;
        }
        if roots.len() != 1 {
            errors.push(MappingValidationError::AmbiguousCrate {
                mapping: mapping.id().clone(),
                crate_name: crate_name.to_owned(),
            });
            continue;
        }

        let rust_path = mapping.selector().canonical_rust_path();
        let Some((kind, def_id)) = resolve_canonical_definition(tcx, roots[0], &rust_path) else {
            errors.push(MappingValidationError::MissingDefinition {
                mapping: mapping.id().clone(),
                rust_path,
            });
            continue;
        };
        if !matches!(
            kind,
            DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::TyAlias
        ) {
            errors.push(MappingValidationError::NotAType {
                mapping: mapping.id().clone(),
                rust_path,
                kind,
                def_id,
            });
            continue;
        }

        let actual = tcx
            .generics_of(def_id)
            .own_params
            .iter()
            .filter(|parameter| matches!(parameter.kind, ty::GenericParamDefKind::Type { .. }))
            .map(|parameter| parameter.name.to_string())
            .collect::<Vec<_>>();
        if mapping.parameters() != actual {
            errors.push(MappingValidationError::GenericParameters {
                mapping: mapping.id().clone(),
                rust_path,
                expected: mapping.parameters().to_vec(),
                actual,
                def_id,
            });
            continue;
        }

        if mlua_typegen::typemap::has_native_mapping(&rust_path) {
            errors.push(MappingValidationError::NativeConflict {
                mapping: mapping.id().clone(),
                rust_path,
                def_id,
            });
        }
    }
    errors
}

fn crate_roots(tcx: TyCtxt<'_>, requested: &str) -> Vec<DefId> {
    let local = (tcx.crate_name(LOCAL_CRATE).as_str() == requested)
        .then_some(LOCAL_CRATE.as_def_id())
        .into_iter();
    local
        .chain(
            tcx.crates(())
                .iter()
                .copied()
                .filter(|crate_num| tcx.crate_name(*crate_num).as_str() == requested)
                .map(|crate_num| crate_num.as_def_id()),
        )
        .collect()
}

fn resolve_canonical_definition(
    tcx: TyCtxt<'_>,
    root: DefId,
    expected_path: &str,
) -> Option<(DefKind, DefId)> {
    if root.is_local() {
        let prefix = format!("{}::", tcx.crate_name(LOCAL_CRATE));
        let expected_path = expected_path.strip_prefix(&prefix).unwrap_or(expected_path);
        return tcx.hir_crate_items(()).definitions().find_map(|local| {
            let def_id = local.to_def_id();
            (tcx.def_path_str(def_id) == expected_path).then(|| (tcx.def_kind(def_id), def_id))
        });
    }

    let mut modules = vec![root];
    while let Some(module) = modules.pop() {
        for child in tcx.module_children(module) {
            let Res::Def(kind, def_id) = child.res else {
                continue;
            };
            if tcx.def_path_str(def_id) == expected_path {
                return Some((kind, def_id));
            }
            if kind.is_module_like() {
                modules.push(def_id);
            }
        }
    }
    None
}
