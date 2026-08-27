//! Private protocol between rustc driver processes and the generation coordinator.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{Dialect, ExtractedUnit, codegen, config::MappingId};

pub(crate) const COMPLETE_MARKER: &str = ".complete";
pub(crate) const MAPPING_USAGE_FILE: &str = ".mappings.json";
pub(crate) const UNIT_MANIFEST_FILE: &str = ".unit.json";
pub const INVOCATION_LEDGER_ENV: &str = "MLUA_TYPEGEN_INVOCATION_LEDGER";
pub const EVIDENCE_SNAPSHOT_ENV: &str = "MLUA_TYPEGEN_EVIDENCE_SNAPSHOT";
pub const FORCE_ANALYZE_ENV: &str = "MLUA_TYPEGEN_FORCE_ANALYZE";
pub const EXTRACTION_SCOPE_ENV: &str = "MLUA_TYPEGEN_EXTRACTION_SCOPE";
pub const REPLAY_GENERATION_ENV: &str = "MLUA_TYPEGEN_REPLAY_GENERATION";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InvocationRecord {
    pub stable_crate_id: u64,
    pub crate_name: String,
    pub compiler_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct RequiredEvidence {
    pub stable_crate_id: u64,
    pub rust_path: String,
    pub lua_path: String,
    pub direction: EvidenceDirection,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum EvidenceDirection {
    FromLua,
    IntoLua,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnitManifest {
    pub stable_crate_id: u64,
    pub crate_name: String,
    pub generation: u32,
    pub scope: Option<BTreeSet<String>>,
    pub pending: bool,
    pub requirements: BTreeSet<RequiredEvidence>,
    pub provided_classes: BTreeMap<String, String>,
    pub conversions: Vec<crate::ConversionContract>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EvidenceCatalog {
    pub classes: BTreeMap<String, String>,
    pub conversions: BTreeMap<String, crate::ConversionContract>,
}

#[derive(Debug)]
pub struct UnitReport {
    pub path: PathBuf,
    pub crate_name: String,
    pub item_count: usize,
}

/// Writes one compiler invocation into a private directory and commits it by
/// creating the completion marker last. The coordinator ignores uncommitted
/// directories left behind by interrupted compiler processes.
pub fn write_unit_report(
    report_root: &Path,
    unit: &ExtractedUnit,
    used_mappings: &std::collections::BTreeSet<MappingId>,
    manifest: &UnitManifest,
    dialect: Dialect,
    crate_name: &str,
) -> Result<UnitReport, ReportError> {
    std::fs::create_dir_all(report_root)?;
    let report = tempfile::Builder::new()
        .prefix(&format!("{crate_name}-"))
        .tempdir_in(report_root)?;

    codegen::write_stubs_for(report.path(), unit, dialect)?;
    codegen::append_event_emissions(report.path(), &unit.events)?;
    let usage = serde_json::to_vec(used_mappings)?;
    let mut usage_file = std::fs::File::create(report.path().join(MAPPING_USAGE_FILE))?;
    usage_file.write_all(&usage)?;
    usage_file.sync_all()?;
    let manifest_bytes = serde_json::to_vec(manifest)?;
    let mut manifest_file = std::fs::File::create(report.path().join(UNIT_MANIFEST_FILE))?;
    manifest_file.write_all(&manifest_bytes)?;
    manifest_file.sync_all()?;

    let item_count = unit.api.item_count();
    let mut marker = std::fs::File::create(report.path().join(COMPLETE_MARKER))?;
    writeln!(marker, "{crate_name}")?;
    marker.sync_all()?;

    let path = report.keep();
    Ok(UnitReport {
        path,
        crate_name: crate_name.to_owned(),
        item_count,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Write(#[from] codegen::WriteError),
    #[error(transparent)]
    EncodeMappingUsage(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use crate::{ExtractedApi, ExtractedUnit, LuaModule, Provenance};

    use super::*;

    #[test]
    fn lua_names_never_control_unit_report_paths() {
        let root = tempfile::tempdir().unwrap();
        let mut extracted = ExtractedApi::new(Provenance::explicit("test"));
        extracted.modules.push(LuaModule {
            name: "../../outside".to_string(),
            doc: None,
            functions: Vec::new(),
            fields: Vec::new(),
        });
        let unit = ExtractedUnit::assemble(extracted).unwrap();

        let report = write_unit_report(
            root.path(),
            &unit,
            &std::collections::BTreeSet::new(),
            &UnitManifest {
                stable_crate_id: 1,
                crate_name: "crate".to_owned(),
                generation: 0,
                scope: None,
                pending: false,
                requirements: BTreeSet::new(),
                provided_classes: BTreeMap::new(),
                conversions: Vec::new(),
            },
            Dialect::LuaLs,
            "crate",
        )
        .unwrap();

        assert!(report.path.join("annotations.lua").is_file());
        assert!(!root.path().join("outside.lua").exists());
        let lua_files = std::fs::read_dir(&report.path)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lua"))
            .count();
        assert_eq!(lua_files, 1);
    }
}
