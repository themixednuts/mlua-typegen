use std::collections::BTreeMap;
use std::fmt;

use mlua_typegen::{ConversionContract, LuaValueDomain};
use rustc_hir::def_id::LocalDefId;
use rustc_middle::ty::TyCtxt;

use super::{lower::alias_def_id, qualified_def_path_str};

mod mir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConversionDirection {
    IntoLua,
    FromLua,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UnsupportedConversion {
    MissingValueEvidence,
    UnsupportedValueVariant,
    DynamicTableKey,
    UnavailableValueProducer,
    AnalysisBudgetExceeded,
}

impl fmt::Display for UnsupportedConversion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingValueEvidence => "no supported Lua value operation reaches success",
            Self::UnsupportedValueVariant => "an unsupported mlua::Value variant reaches success",
            Self::DynamicTableKey => "a table shape depends on a dynamic key",
            Self::UnavailableValueProducer => "a value-producing helper body is unavailable",
            Self::AnalysisBudgetExceeded => "the local-helper analysis budget was exhausted",
        })
    }
}

impl UnsupportedConversion {
    pub(super) const fn remediation(self) -> &'static str {
        match self {
            Self::MissingValueEvidence => {
                "return a statically visible mlua::Value variant on every successful path"
            }
            Self::UnsupportedValueVariant => {
                "replace the unsupported value variant or add exact extraction support for it"
            }
            Self::DynamicTableKey => {
                "use statically known table keys or declare an explicit exact type mapping"
            }
            Self::UnavailableValueProducer => {
                "make the helper source available or declare an explicit exact type mapping"
            }
            Self::AnalysisBudgetExceeded => {
                "reduce helper indirection or declare an explicit exact type mapping"
            }
        }
    }
}

impl fmt::Display for ConversionDirection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntoLua => formatter.write_str("IntoLua"),
            Self::FromLua => formatter.write_str("FromLua"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ConversionSubject {
    pub(super) impl_def_id: LocalDefId,
    pub(super) method_def_id: LocalDefId,
    pub(super) direction: ConversionDirection,
}

impl ConversionSubject {
    pub(super) fn from_impl(
        tcx: TyCtxt<'_>,
        impl_def_id: LocalDefId,
        direction: ConversionDirection,
    ) -> Option<Self> {
        let method_name = match direction {
            ConversionDirection::IntoLua => "into_lua",
            ConversionDirection::FromLua => "from_lua",
        };
        let method_def_id = tcx
            .associated_items(impl_def_id.to_def_id())
            .in_definition_order()
            .find(|item| item.name().as_str() == method_name)?
            .def_id
            .as_local()?;

        Some(Self {
            impl_def_id,
            method_def_id,
            direction,
        })
    }

    pub(super) fn rust_path(self, tcx: TyCtxt<'_>) -> String {
        conversion_rust_path(tcx, tcx.type_of(self.impl_def_id).skip_binder())
    }
}

#[derive(Debug)]
pub(super) struct ConversionError {
    pub(super) impl_def_id: LocalDefId,
    pub(super) method_def_id: LocalDefId,
    pub(super) rust_type: String,
    pub(super) direction: ConversionDirection,
    pub(super) reason: UnsupportedConversion,
}

#[derive(Debug)]
pub(super) struct ConversionAnalysis {
    pub(super) rust_path: String,
    pub(super) rust_type: String,
    pub(super) direction: ConversionDirection,
    pub(super) domain: LuaValueDomain,
}

impl fmt::Display for ConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot statically analyze {} for `{}`: {}",
            self.direction, self.rust_type, self.reason
        )
    }
}

impl std::error::Error for ConversionError {}

pub(super) fn analyze(
    tcx: TyCtxt<'_>,
    subject: ConversionSubject,
) -> Result<ConversionAnalysis, ConversionError> {
    let self_ty = tcx.type_of(subject.impl_def_id).skip_binder();
    let rust_type = self_ty.to_string();
    let rust_path = conversion_rust_path(tcx, self_ty);
    let body = tcx.optimized_mir(subject.method_def_id);
    mir::analyze_domain(tcx, body, &rust_type, subject.direction)
        .map(|domain| ConversionAnalysis {
            rust_path,
            rust_type: rust_type.clone(),
            direction: subject.direction,
            domain,
        })
        .map_err(|reason| ConversionError {
            impl_def_id: subject.impl_def_id,
            method_def_id: subject.method_def_id,
            rust_type,
            direction: subject.direction,
            reason,
        })
}

fn conversion_rust_path(tcx: TyCtxt<'_>, self_ty: rustc_middle::ty::Ty<'_>) -> String {
    match self_ty.kind() {
        rustc_middle::ty::TyKind::Adt(definition, _) => {
            qualified_def_path_str(tcx, definition.did())
        }
        #[cfg(rustc_alias_is_rigid)]
        rustc_middle::ty::TyKind::Alias(_, alias) => {
            qualified_def_path_str(tcx, alias_def_id(alias))
        }
        #[cfg(not(rustc_alias_is_rigid))]
        rustc_middle::ty::TyKind::Alias(alias) => qualified_def_path_str(tcx, alias_def_id(alias)),
        _ => self_ty.to_string(),
    }
}

pub(super) fn assemble(
    analyses: impl IntoIterator<Item = ConversionAnalysis>,
) -> Vec<ConversionContract> {
    let mut contracts = BTreeMap::<String, ConversionContract>::new();
    for analysis in analyses {
        let contract = contracts
            .entry(analysis.rust_path.clone())
            .or_insert_with(|| ConversionContract {
                rust_path: analysis.rust_path,
                rust_type: analysis.rust_type,
                into_lua: None,
                from_lua: None,
            });
        match analysis.direction {
            ConversionDirection::IntoLua => contract.into_lua = Some(analysis.domain),
            ConversionDirection::FromLua => contract.from_lua = Some(analysis.domain),
        }
    }
    contracts.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembly_preserves_directions() {
        let contracts = assemble([
            ConversionAnalysis {
                rust_path: "fixture::Mode".to_owned(),
                rust_type: "Mode".to_owned(),
                direction: ConversionDirection::FromLua,
                domain: LuaValueDomain::String,
            },
            ConversionAnalysis {
                rust_path: "fixture::Mode".to_owned(),
                rust_type: "Mode".to_owned(),
                direction: ConversionDirection::IntoLua,
                domain: LuaValueDomain::Integer,
            },
        ]);

        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts[0].rust_path, "fixture::Mode");
        assert_eq!(contracts[0].into_lua, Some(LuaValueDomain::Integer));
        assert_eq!(contracts[0].from_lua, Some(LuaValueDomain::String));
    }
}
