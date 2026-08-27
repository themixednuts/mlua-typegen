use mlua_typegen::config::ResolvedConfig;
use mlua_typegen::driver_protocol::EvidenceCatalog;
use mlua_typegen::{ExtractedApi, ExtractedUnit, Provenance};
use rustc_middle::ty::TyCtxt;

use super::CompiledUnit;
use super::diagnostic::{CompileDiagnostic, CompileDiagnostics};
use super::plan::{ExtractionPlan, ExtractionWork};

pub(super) struct ApiCompiler<'config, 'tcx> {
    tcx: TyCtxt<'tcx>,
    config: &'config ResolvedConfig,
    evidence: &'config EvidenceCatalog,
    scope: Option<&'config std::collections::BTreeSet<String>>,
}

impl<'config, 'tcx> ApiCompiler<'config, 'tcx> {
    pub(super) fn new(
        tcx: TyCtxt<'tcx>,
        config: &'config ResolvedConfig,
        evidence: &'config EvidenceCatalog,
        scope: Option<&'config std::collections::BTreeSet<String>>,
    ) -> Self {
        Self {
            tcx,
            config,
            evidence,
            scope,
        }
    }

    pub(super) fn compile(self) -> Result<CompiledUnit, CompileDiagnostics> {
        let mut diagnostics = super::mapping_validation::validate(self.tcx, self.config)
            .into_iter()
            .map(|error| CompileDiagnostic::mapping_validation(self.tcx, error))
            .collect::<Vec<_>>();
        let plan = self.plan();
        let (mut api, provided_classes, emission_diagnostics) = self.emit(plan);
        diagnostics.extend(emission_diagnostics);
        let finish = self.finish(&mut api, &mut diagnostics);
        let unit = match ExtractedUnit::assemble(api) {
            Ok(unit) => Some(unit),
            Err(errors) => {
                diagnostics.extend(
                    errors
                        .iter()
                        .cloned()
                        .map(|error| CompileDiagnostic::semantic(self.tcx, error)),
                );
                None
            }
        };

        if diagnostics.is_empty() {
            Ok(CompiledUnit {
                unit: unit.expect("successful semantic assembly produced a unit"),
                used_mappings: finish.used_mappings,
                requirements: finish.requirements,
                provided_classes,
            })
        } else {
            Err(CompileDiagnostics::from_unsorted(diagnostics))
        }
    }

    fn plan(&self) -> ExtractionPlan<'tcx> {
        ExtractionPlan::for_crate(self.tcx, self.scope)
    }

    fn emit(
        &self,
        plan: ExtractionPlan<'tcx>,
    ) -> (
        ExtractedApi,
        std::collections::BTreeMap<String, String>,
        Vec<CompileDiagnostic>,
    ) {
        let crate_name = self.tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE);
        let mut api = ExtractedApi::new(Provenance::rust_crate(crate_name.as_str()));
        let mut conversions = Vec::new();
        let mut diagnostics = Vec::new();
        let mut provided_classes = std::collections::BTreeMap::new();

        for work in plan.into_work() {
            self.emit_work(
                work,
                &mut api,
                &mut conversions,
                &mut provided_classes,
                &mut diagnostics,
            );
        }

        api.conversions = super::conversion::assemble(conversions);
        (api, provided_classes, diagnostics)
    }

    fn finish(
        &self,
        api: &mut ExtractedApi,
        diagnostics: &mut Vec<CompileDiagnostic>,
    ) -> super::finish::FinishOutcome {
        match super::finish::finish_api(api, self.config, self.evidence) {
            Ok(finish) => finish,
            Err(errors) => {
                diagnostics.extend(
                    errors
                        .into_iter()
                        .map(|error| CompileDiagnostic::type_resolution(self.tcx, error)),
                );
                super::finish::FinishOutcome {
                    used_mappings: std::collections::BTreeSet::new(),
                    requirements: std::collections::BTreeSet::new(),
                }
            }
        }
    }

    fn emit_work(
        &self,
        work: ExtractionWork<'tcx>,
        api: &mut ExtractedApi,
        conversions: &mut Vec<super::conversion::ConversionAnalysis>,
        provided_classes: &mut std::collections::BTreeMap<String, String>,
        diagnostics: &mut Vec<CompileDiagnostic>,
    ) {
        match work {
            ExtractionWork::Class { owner } => {
                provided_classes.insert(owner.rust_path(self.tcx), owner.lua_name().to_owned());
                super::upsert_class(api, owner.empty_class(self.tcx));
            }
            ExtractionWork::Conversion(subject) => {
                match super::conversion::analyze(self.tcx, subject) {
                    Ok(conversion) => conversions.push(conversion),
                    Err(error) => {
                        diagnostics.push(CompileDiagnostic::conversion(self.tcx, error));
                    }
                }
            }
            ExtractionWork::Body {
                body_id,
                module_item,
                generated_module_name,
                registration_owner,
            } => {
                if let Some(owner) = registration_owner.as_ref() {
                    provided_classes.insert(owner.rust_path(self.tcx), owner.lua_name().to_owned());
                }
                super::emit::collect_body(
                    self.tcx,
                    body_id,
                    module_item,
                    generated_module_name,
                    registration_owner,
                    api,
                );
            }
        }
    }
}
