use std::fmt;

use mlua_typegen::SemanticError;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_middle::ty::TyCtxt;
use rustc_span::Span;

use super::conversion::{ConversionError, UnsupportedConversion};
use super::finish::TypeResolutionError;
use super::mapping_validation::MappingValidationError;
use super::source::qualified_def_path_str;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DiagnosticCode {
    UnsupportedConversion,
    UnresolvedType,
    InvalidMapping,
    ConflictingType,
    ConflictingValue,
    GenericArity,
}

impl DiagnosticCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedConversion => "MLTG1001",
            Self::UnresolvedType => "MLTG1002",
            Self::InvalidMapping => "MLTG1003",
            Self::ConflictingType => "MLTG2001",
            Self::ConflictingValue => "MLTG2002",
            Self::GenericArity => "MLTG2003",
        }
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DiagnosticStage {
    Extraction,
    SemanticValidation,
}

impl fmt::Display for DiagnosticStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Extraction => "extraction",
            Self::SemanticValidation => "semantic validation",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DiagnosticReason {
    UnsupportedConversion(UnsupportedConversion),
    TypeResolution(TypeResolutionError),
    MappingValidation(MappingValidationError),
    Semantic(SemanticError),
}

impl fmt::Display for DiagnosticReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedConversion(reason) => fmt::Display::fmt(reason, formatter),
            Self::TypeResolution(reason) => fmt::Display::fmt(reason, formatter),
            Self::MappingValidation(reason) => fmt::Display::fmt(reason, formatter),
            Self::Semantic(reason) => fmt::Display::fmt(reason, formatter),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompileDiagnostic {
    code: DiagnosticCode,
    stage: DiagnosticStage,
    primary_span: Span,
    rust_owner: String,
    lua_path: Option<String>,
    reason: DiagnosticReason,
    remediation: &'static str,
}

impl CompileDiagnostic {
    pub(super) fn conversion(tcx: TyCtxt<'_>, error: ConversionError) -> Self {
        Self {
            code: DiagnosticCode::UnsupportedConversion,
            stage: DiagnosticStage::Extraction,
            primary_span: tcx.def_span(error.method_def_id),
            rust_owner: qualified_def_path_str(tcx, error.impl_def_id.to_def_id()),
            lua_path: None,
            remediation: error.reason.remediation(),
            reason: DiagnosticReason::UnsupportedConversion(error.reason),
        }
    }

    pub(super) fn semantic(tcx: TyCtxt<'_>, error: SemanticError) -> Self {
        let (code, lua_path, remediation) = match &error {
            SemanticError::ConflictingType { id, .. } => (
                DiagnosticCode::ConflictingType,
                Some(id.to_string()),
                "rename or merge the declarations so the Lua type has one compatible definition",
            ),
            SemanticError::ConflictingValue { path, .. } => (
                DiagnosticCode::ConflictingValue,
                Some(path.to_string()),
                "rename or merge the registrations so the Lua path has one compatible definition",
            ),
            SemanticError::GenericArity { referenced, .. } => (
                DiagnosticCode::GenericArity,
                Some(referenced.to_string()),
                "supply exactly the declared number of generic type arguments",
            ),
        };
        let rust_owner = tcx.crate_name(LOCAL_CRATE).as_str().to_owned();
        Self {
            code,
            stage: DiagnosticStage::SemanticValidation,
            primary_span: tcx.def_span(LOCAL_CRATE.as_def_id()),
            rust_owner,
            lua_path,
            reason: DiagnosticReason::Semantic(error),
            remediation,
        }
    }

    pub(super) fn type_resolution(tcx: TyCtxt<'_>, error: TypeResolutionError) -> Self {
        let primary_span = local_definition_span(tcx, error.rust_path())
            .unwrap_or_else(|| tcx.def_span(LOCAL_CRATE.as_def_id()));
        Self {
            code: DiagnosticCode::UnresolvedType,
            stage: DiagnosticStage::Extraction,
            primary_span,
            rust_owner: error.rust_path().to_owned(),
            lua_path: Some(error.lua_path().to_owned()),
            remediation: error.remediation(),
            reason: DiagnosticReason::TypeResolution(error),
        }
    }

    pub(super) fn mapping_validation(tcx: TyCtxt<'_>, error: MappingValidationError) -> Self {
        let primary_span = error
            .def_id()
            .map(|def_id| tcx.def_span(def_id))
            .unwrap_or_else(|| tcx.def_span(LOCAL_CRATE.as_def_id()));
        Self {
            code: DiagnosticCode::InvalidMapping,
            stage: DiagnosticStage::Extraction,
            primary_span,
            rust_owner: error.rust_path().to_owned(),
            lua_path: None,
            remediation: error.remediation(),
            reason: DiagnosticReason::MappingValidation(error),
        }
    }

    pub(crate) const fn code(&self) -> DiagnosticCode {
        self.code
    }

    pub(crate) const fn stage(&self) -> DiagnosticStage {
        self.stage
    }

    pub(crate) const fn primary_span(&self) -> Span {
        self.primary_span
    }

    pub(crate) fn rust_owner(&self) -> &str {
        &self.rust_owner
    }

    pub(crate) fn lua_path(&self) -> Option<&str> {
        self.lua_path.as_deref()
    }

    pub(crate) fn reason(&self) -> impl fmt::Display + '_ {
        &self.reason
    }

    pub(crate) const fn remediation(&self) -> &'static str {
        self.remediation
    }

    fn sort_key(&self) -> (u32, DiagnosticCode, DiagnosticStage, &str, Option<&str>) {
        (
            self.primary_span.lo().0,
            self.code,
            self.stage,
            &self.rust_owner,
            self.lua_path.as_deref(),
        )
    }
}

fn local_definition_span(tcx: TyCtxt<'_>, rust_path: &str) -> Option<Span> {
    tcx.hir_crate_items(())
        .definitions()
        .map(|owner| owner.to_def_id())
        .find(|def_id| qualified_def_path_str(tcx, *def_id) == rust_path)
        .map(|def_id| tcx.def_span(def_id))
}

#[derive(Debug)]
pub(crate) struct CompileDiagnostics(Vec<CompileDiagnostic>);

impl CompileDiagnostics {
    pub(super) fn from_unsorted(mut diagnostics: Vec<CompileDiagnostic>) -> Self {
        diagnostics.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        Self(diagnostics)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &CompileDiagnostic> {
        self.0.iter()
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for CompileDiagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} extraction error(s)", self.len())
    }
}

impl std::error::Error for CompileDiagnostics {}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_span::DUMMY_SP;

    fn fixture(code: DiagnosticCode, owner: &str) -> CompileDiagnostic {
        CompileDiagnostic {
            code,
            stage: DiagnosticStage::Extraction,
            primary_span: DUMMY_SP,
            rust_owner: owner.to_owned(),
            lua_path: None,
            reason: DiagnosticReason::UnsupportedConversion(
                UnsupportedConversion::MissingValueEvidence,
            ),
            remediation: "fixture",
        }
    }

    #[test]
    fn diagnostics_are_sorted_by_stable_fields() {
        let diagnostics = CompileDiagnostics::from_unsorted(vec![
            fixture(DiagnosticCode::UnsupportedConversion, "zeta"),
            fixture(DiagnosticCode::GenericArity, "alpha"),
            fixture(DiagnosticCode::UnsupportedConversion, "alpha"),
        ]);
        let keys = diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code(), diagnostic.rust_owner()))
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            [
                (DiagnosticCode::UnsupportedConversion, "alpha"),
                (DiagnosticCode::UnsupportedConversion, "zeta"),
                (DiagnosticCode::GenericArity, "alpha"),
            ]
        );
    }
}
