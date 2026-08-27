//! Deterministic assembly and atomic publication of generated Lua types.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use fs2::FileExt as _;
use thiserror::Error;

use crate::{
    Dialect, codegen,
    config::{self, MappingId},
    driver_protocol::{
        COMPLETE_MARKER, EVIDENCE_SNAPSHOT_ENV, EvidenceCatalog, FORCE_ANALYZE_ENV,
        INVOCATION_LEDGER_ENV, InvocationRecord, MAPPING_USAGE_FILE, REPLAY_GENERATION_ENV,
        UNIT_MANIFEST_FILE, UnitManifest,
    },
};

const SNAPSHOT_FILE: &str = "mlua-typegen.lua";
const LOCK_FILE: &str = ".mlua-typegen.lock";

#[derive(Debug)]
pub struct RunWorkspace {
    _root: tempfile::TempDir,
    report_dir: PathBuf,
    cargo_target_dir: PathBuf,
    config_snapshot: PathBuf,
    invocation_ledger: PathBuf,
    evidence_snapshot: PathBuf,
}

impl RunWorkspace {
    pub fn create() -> Result<Self, WorkspaceError> {
        let root = tempfile::Builder::new()
            .prefix("mlua-typegen-")
            .tempdir()
            .map_err(WorkspaceError::Create)?;
        let report_dir = root.path().join("reports");
        std::fs::create_dir(&report_dir).map_err(WorkspaceError::CreateReports)?;
        let cargo_target_dir = root.path().join("cargo-target");
        let config_snapshot = root.path().join("resolved-config.json");
        let invocation_ledger = root.path().join("invocations");
        std::fs::create_dir(&invocation_ledger).map_err(WorkspaceError::CreateInvocations)?;
        let evidence_snapshot = root.path().join("evidence.json");
        Ok(Self {
            _root: root,
            report_dir,
            cargo_target_dir,
            config_snapshot,
            invocation_ledger,
            evidence_snapshot,
        })
    }

    #[must_use]
    pub fn report_dir(&self) -> &Path {
        &self.report_dir
    }

    #[must_use]
    pub fn cargo_target_dir(&self) -> &Path {
        &self.cargo_target_dir
    }

    pub fn snapshot_config(&self, bytes: &[u8]) -> Result<&Path, WorkspaceError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.config_snapshot)
            .map_err(WorkspaceError::CreateConfigSnapshot)?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(WorkspaceError::WriteConfigSnapshot)?;
        Ok(&self.config_snapshot)
    }

    #[must_use]
    pub fn invocation_ledger(&self) -> &Path {
        &self.invocation_ledger
    }

    pub fn write_evidence(&self, bytes: &[u8]) -> Result<&Path, WorkspaceError> {
        std::fs::write(&self.evidence_snapshot, bytes)
            .map_err(WorkspaceError::WriteEvidenceSnapshot)?;
        Ok(&self.evidence_snapshot)
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("failed to create a private generation workspace")]
    Create(#[source] io::Error),
    #[error("failed to create the private unit-report directory")]
    CreateReports(#[source] io::Error),
    #[error("failed to create the immutable mapping-configuration snapshot")]
    CreateConfigSnapshot(#[source] io::Error),
    #[error("failed to write the immutable mapping-configuration snapshot")]
    WriteConfigSnapshot(#[source] io::Error),
    #[error("failed to create the compiler invocation ledger")]
    CreateInvocations(#[source] io::Error),
    #[error("failed to write the dependency-evidence snapshot")]
    WriteEvidenceSnapshot(#[source] io::Error),
}

#[derive(Debug, Error)]
pub enum ReachabilityError {
    #[error("failed to enumerate compiler unit reports in `{}`", path.display())]
    ReadReports {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read compiler unit manifest `{}`", path.display())]
    ReadManifest {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to decode compiler unit manifest `{}`", path.display())]
    DecodeManifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "dependency evidence for `{rust_path}` at Lua path `{lua_path}` is required, but Cargo did not compile its owning crate ({stable_crate_id:016x}) from locally available source"
    )]
    SourceUnavailable {
        stable_crate_id: u64,
        rust_path: String,
        lua_path: String,
    },
    #[error("failed to read recorded compiler invocation `{}`", path.display())]
    ReadInvocation {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to decode recorded compiler invocation `{}`", path.display())]
    DecodeInvocation {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to encode dependency evidence")]
    EncodeEvidence(#[source] serde_json::Error),
    #[error("failed to write dependency evidence")]
    WriteEvidence(#[source] WorkspaceError),
    #[error("failed to write extraction scope `{}`", path.display())]
    WriteScope {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to start compiler replay for `{crate_name}`")]
    StartReplay {
        crate_name: String,
        #[source]
        source: io::Error,
    },
    #[error("compiler replay for `{crate_name}` failed with {status}")]
    ReplayFailed {
        crate_name: String,
        status: ExitStatus,
    },
    #[error("dependency evidence did not converge after {rounds} compiler replay rounds")]
    DidNotConverge { rounds: usize },
    #[error("conflicting replayed class names for `{rust_path}`: `{first}` and `{second}`")]
    ConflictingClass {
        rust_path: String,
        first: String,
        second: String,
    },
    #[error("conflicting replayed conversion contracts for `{rust_path}`")]
    ConflictingConversion { rust_path: String },
}

/// Resolves source-backed dependency types to a fixed point by replaying the
/// exact rustc invocations Cargo used during the initial check.
pub fn resolve_reachable_reports(
    workspace: &RunWorkspace,
    driver: &Path,
    config_snapshot: &Path,
    dialect: Dialect,
) -> Result<(), ReachabilityError> {
    const MAX_ROUNDS: usize = 64;

    for round in 0..MAX_ROUNDS {
        let reports = read_unit_manifests(workspace.report_dir())?;
        let active = active_manifests(&reports);
        let evidence = evidence_catalog(active.values().copied())?;
        let pending = active
            .values()
            .copied()
            .filter(|(_, manifest)| manifest.pending)
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Ok(());
        }

        let evidence_bytes =
            serde_json::to_vec(&evidence).map_err(ReachabilityError::EncodeEvidence)?;
        let evidence_path = workspace
            .write_evidence(&evidence_bytes)
            .map_err(ReachabilityError::WriteEvidence)?;

        let mut replays = BTreeMap::<u64, Option<BTreeSet<String>>>::new();
        for (_, manifest) in &pending {
            let mut missing = false;
            for requirement in &manifest.requirements {
                if evidence_satisfies(&evidence, requirement) {
                    continue;
                }
                missing = true;
                let invocation_path = invocation_path(workspace, requirement.stable_crate_id);
                if !invocation_path.is_file() {
                    return Err(ReachabilityError::SourceUnavailable {
                        stable_crate_id: requirement.stable_crate_id,
                        rust_path: requirement.rust_path.clone(),
                        lua_path: requirement.lua_path.clone(),
                    });
                }
                let mut requested = BTreeSet::new();
                requested.insert(requirement.rust_path.clone());
                replays
                    .entry(requirement.stable_crate_id)
                    .and_modify(|scope| {
                        if let Some(scope) = scope {
                            scope.extend(requested.iter().cloned());
                        }
                    })
                    .or_insert(Some(requested));
            }
            if !missing {
                replays.insert(manifest.stable_crate_id, manifest.scope.clone());
            }
        }

        for (stable_crate_id, scope) in replays {
            replay_invocation(
                workspace,
                driver,
                config_snapshot,
                evidence_path,
                dialect,
                ReplayRequest {
                    stable_crate_id,
                    scope: scope.as_ref(),
                    generation: u32::try_from(round + 1).unwrap_or(u32::MAX),
                },
            )?;
        }

        if round + 1 == MAX_ROUNDS {
            return Err(ReachabilityError::DidNotConverge { rounds: MAX_ROUNDS });
        }
    }
    unreachable!("the replay loop either converges or reaches its bound")
}

type ManifestReport = (PathBuf, UnitManifest);

fn read_unit_manifests(report_root: &Path) -> Result<Vec<ManifestReport>, ReachabilityError> {
    let entries =
        std::fs::read_dir(report_root).map_err(|source| ReachabilityError::ReadReports {
            path: report_root.to_owned(),
            source,
        })?;
    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ReachabilityError::ReadReports {
            path: report_root.to_owned(),
            source,
        })?;
        let report = entry.path();
        if !entry
            .file_type()
            .map_err(|source| ReachabilityError::ReadReports {
                path: report.clone(),
                source,
            })?
            .is_dir()
            || !report.join(COMPLETE_MARKER).is_file()
        {
            continue;
        }
        let path = report.join(UNIT_MANIFEST_FILE);
        let bytes = std::fs::read(&path).map_err(|source| ReachabilityError::ReadManifest {
            path: path.clone(),
            source,
        })?;
        let manifest =
            serde_json::from_slice(&bytes).map_err(|source| ReachabilityError::DecodeManifest {
                path: path.clone(),
                source,
            })?;
        manifests.push((report, manifest));
    }
    Ok(manifests)
}

fn active_manifests(reports: &[ManifestReport]) -> BTreeMap<u64, &ManifestReport> {
    let mut active = BTreeMap::new();
    for report in reports {
        active
            .entry(report.1.stable_crate_id)
            .and_modify(|current: &mut &ManifestReport| {
                if report.1.generation > current.1.generation
                    || (report.1.generation == current.1.generation
                        && current.1.pending
                        && !report.1.pending)
                {
                    *current = report;
                }
            })
            .or_insert(report);
    }
    active
}

fn evidence_catalog<'a>(
    manifests: impl IntoIterator<Item = &'a ManifestReport>,
) -> Result<EvidenceCatalog, ReachabilityError> {
    let mut catalog = EvidenceCatalog::default();
    for (_, manifest) in manifests {
        if manifest.pending {
            continue;
        }
        for (rust_path, class_name) in &manifest.provided_classes {
            if let Some(previous) = catalog
                .classes
                .insert(rust_path.clone(), class_name.clone())
                && previous != *class_name
            {
                return Err(ReachabilityError::ConflictingClass {
                    rust_path: rust_path.clone(),
                    first: previous,
                    second: class_name.clone(),
                });
            }
        }
        for contract in &manifest.conversions {
            if let Some(previous) = catalog
                .conversions
                .insert(contract.rust_path.clone(), contract.clone())
                && previous != *contract
            {
                return Err(ReachabilityError::ConflictingConversion {
                    rust_path: contract.rust_path.clone(),
                });
            }
        }
    }
    Ok(catalog)
}

fn evidence_satisfies(
    evidence: &EvidenceCatalog,
    requirement: &crate::driver_protocol::RequiredEvidence,
) -> bool {
    if evidence.classes.contains_key(&requirement.rust_path) {
        return true;
    }
    evidence
        .conversions
        .get(&requirement.rust_path)
        .is_some_and(|contract| match requirement.direction {
            crate::driver_protocol::EvidenceDirection::FromLua => contract.from_lua.is_some(),
            crate::driver_protocol::EvidenceDirection::IntoLua => contract.into_lua.is_some(),
        })
}

fn invocation_path(workspace: &RunWorkspace, stable_crate_id: u64) -> PathBuf {
    workspace
        .invocation_ledger()
        .join(format!("{stable_crate_id:016x}.json"))
}

struct ReplayRequest<'a> {
    stable_crate_id: u64,
    scope: Option<&'a BTreeSet<String>>,
    generation: u32,
}

fn replay_invocation(
    workspace: &RunWorkspace,
    driver: &Path,
    config_snapshot: &Path,
    evidence_snapshot: &Path,
    dialect: Dialect,
    request: ReplayRequest<'_>,
) -> Result<(), ReachabilityError> {
    let path = invocation_path(workspace, request.stable_crate_id);
    let bytes = std::fs::read(&path).map_err(|source| ReachabilityError::ReadInvocation {
        path: path.clone(),
        source,
    })?;
    let invocation: InvocationRecord =
        serde_json::from_slice(&bytes).map_err(|source| ReachabilityError::DecodeInvocation {
            path: path.clone(),
            source,
        })?;
    let mut command = Command::new(driver);
    command
        .args(&invocation.compiler_args)
        .env("MLUA_TYPEGEN_OUTPUT", workspace.report_dir())
        .env(INVOCATION_LEDGER_ENV, workspace.invocation_ledger())
        .env(config::CONFIG_SNAPSHOT_ENV, config_snapshot)
        .env(EVIDENCE_SNAPSHOT_ENV, evidence_snapshot)
        .env(FORCE_ANALYZE_ENV, "1")
        .env(REPLAY_GENERATION_ENV, request.generation.to_string());
    let scope_path = workspace
        .invocation_ledger()
        .join(format!("{:016x}.scope.json", request.stable_crate_id));
    if let Some(scope) = request.scope {
        let bytes = serde_json::to_vec(scope).map_err(ReachabilityError::EncodeEvidence)?;
        std::fs::write(&scope_path, bytes).map_err(|source| ReachabilityError::WriteScope {
            path: scope_path.clone(),
            source,
        })?;
        command.env(crate::driver_protocol::EXTRACTION_SCOPE_ENV, &scope_path);
    } else {
        command.env_remove(crate::driver_protocol::EXTRACTION_SCOPE_ENV);
    }
    if dialect == Dialect::EmmyLua {
        command.env("MLUA_TYPEGEN_EMMYLUA", "1");
    } else {
        command.env_remove("MLUA_TYPEGEN_EMMYLUA");
    }
    let status = command
        .status()
        .map_err(|source| ReachabilityError::StartReplay {
            crate_name: invocation.crate_name.clone(),
            source,
        })?;
    if !status.success() {
        return Err(ReachabilityError::ReplayFailed {
            crate_name: invocation.crate_name,
            status,
        });
    }
    Ok(())
}

#[derive(Debug)]
pub struct OutputLock {
    _file: File,
    output_dir: PathBuf,
}

impl OutputLock {
    pub fn acquire(output_dir: &Path) -> Result<Self, LockError> {
        std::fs::create_dir_all(output_dir).map_err(|source| LockError::CreateOutput {
            path: output_dir.to_owned(),
            source,
        })?;
        let path = output_dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| LockError::Open { path, source })?;
        file.try_lock_exclusive()
            .map_err(|source| LockError::AlreadyRunning {
                path: output_dir.to_owned(),
                source,
            })?;
        Ok(Self {
            _file: file,
            output_dir: output_dir.to_owned(),
        })
    }

    pub fn publish(&self, snapshot: &GeneratedSnapshot) -> Result<PublishedSnapshot, PublishError> {
        publish_snapshot(&self.output_dir, snapshot)
    }

    #[must_use]
    pub fn snapshot_path(&self) -> PathBuf {
        self.output_dir.join(SNAPSHOT_FILE)
    }
}

#[derive(Debug, Error)]
pub enum LockError {
    #[error("failed to create output directory `{}`", path.display())]
    CreateOutput {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to open output lock `{}`", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("another mlua-typegen run is publishing to `{}`", path.display())]
    AlreadyRunning {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedSnapshot {
    content: String,
    unit_count: usize,
    used_mappings: BTreeSet<MappingId>,
}

impl GeneratedSnapshot {
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    #[must_use]
    pub fn unit_count(&self) -> usize {
        self.unit_count
    }

    #[must_use]
    pub fn used_mappings(&self) -> &BTreeSet<MappingId> {
        &self.used_mappings
    }
}

#[derive(Debug, Error)]
pub enum AssembleError {
    #[error("failed to enumerate unit reports in `{}`", path.display())]
    ReadReports {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("no complete compiler unit reports were produced")]
    NoCompleteReports,
    #[error("failed to read generated artifact `{}`", path.display())]
    ReadArtifact {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to decode mapping usage `{}`", path.display())]
    DecodeMappingUsage {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to decode unit manifest `{}`", path.display())]
    DecodeUnitManifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("function `{name}` has conflicting return types: `{first}` and `{second}`")]
    ConflictingFunction {
        name: String,
        first: String,
        second: String,
    },
    #[error("event `{name}` has conflicting callback types: `{first}` and `{second}`")]
    ConflictingEvent {
        name: String,
        first: String,
        second: String,
    },
}

pub fn assemble_reports(report_root: &Path) -> Result<GeneratedSnapshot, AssembleError> {
    let reports = complete_reports(report_root)?;
    if reports.is_empty() {
        return Err(AssembleError::NoCompleteReports);
    }
    let mut active = BTreeMap::<u64, (PathBuf, UnitManifest)>::new();
    for report in reports {
        let manifest_path = report.join(UNIT_MANIFEST_FILE);
        let manifest =
            std::fs::read(&manifest_path).map_err(|source| AssembleError::ReadArtifact {
                path: manifest_path.clone(),
                source,
            })?;
        let manifest: UnitManifest = serde_json::from_slice(&manifest).map_err(|source| {
            AssembleError::DecodeUnitManifest {
                path: manifest_path,
                source,
            }
        })?;
        active
            .entry(manifest.stable_crate_id)
            .and_modify(|current| {
                if manifest.generation > current.1.generation
                    || (manifest.generation == current.1.generation
                        && current.1.pending
                        && !manifest.pending)
                {
                    *current = (report.clone(), manifest.clone());
                }
            })
            .or_insert((report, manifest));
    }

    let mut chunks = BTreeSet::new();
    let mut function_types = BTreeMap::<String, Vec<String>>::new();
    let mut events = BTreeMap::<String, Vec<String>>::new();
    let mut used_mappings = BTreeSet::new();
    let mut included_report_count = 0usize;

    for (report, manifest) in active.values() {
        if manifest.pending {
            continue;
        }
        included_report_count += 1;
        let usage_path = report.join(MAPPING_USAGE_FILE);
        let usage = std::fs::read(&usage_path).map_err(|source| AssembleError::ReadArtifact {
            path: usage_path.clone(),
            source,
        })?;
        let report_usage: BTreeSet<MappingId> =
            serde_json::from_slice(&usage).map_err(|source| AssembleError::DecodeMappingUsage {
                path: usage_path,
                source,
            })?;
        used_mappings.extend(report_usage);
        let report_functions = codegen::try_read_function_types(report).map_err(|source| {
            AssembleError::ReadArtifact {
                path: report.join(".func_types.txt"),
                source,
            }
        })?;
        merge_function_types(&mut function_types, report_functions)?;
        let report_events = codegen::try_read_event_emissions(report).map_err(|source| {
            AssembleError::ReadArtifact {
                path: report.join(".events.txt"),
                source,
            }
        })?;
        merge_events(&mut events, report_events)?;

        let entries = std::fs::read_dir(report).map_err(|source| AssembleError::ReadReports {
            path: report.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| AssembleError::ReadReports {
                path: report.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("lua") {
                continue;
            }
            let content =
                std::fs::read_to_string(&path).map_err(|source| AssembleError::ReadArtifact {
                    path: path.clone(),
                    source,
                })?;
            let body = strip_meta_header(&content);
            if !body.is_empty() {
                chunks.insert(body);
            }
        }
    }

    if !events.is_empty() {
        let events = events.into_iter().collect::<Vec<_>>();
        let body = strip_meta_header(&codegen::generate_event_stubs(&events));
        if !body.is_empty() {
            chunks.insert(body);
        }
    }

    let chunks = chunks
        .into_iter()
        .map(|chunk| {
            resolve_function_lookups(&chunk, &function_types)
                .trim_end_matches('\n')
                .to_owned()
        })
        .collect::<Vec<_>>();
    let mut content = String::from("---@meta\n");
    if !chunks.is_empty() {
        content.push('\n');
        content.push_str(&chunks.join("\n\n"));
        content.push('\n');
    }

    if included_report_count == 0 {
        return Err(AssembleError::NoCompleteReports);
    }

    Ok(GeneratedSnapshot {
        content,
        unit_count: included_report_count,
        used_mappings,
    })
}

fn complete_reports(report_root: &Path) -> Result<Vec<PathBuf>, AssembleError> {
    let entries = std::fs::read_dir(report_root).map_err(|source| AssembleError::ReadReports {
        path: report_root.to_owned(),
        source,
    })?;
    let mut reports = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| AssembleError::ReadReports {
            path: report_root.to_owned(),
            source,
        })?;
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|source| AssembleError::ReadReports {
                path: path.clone(),
                source,
            })?
            .is_dir()
            && path.join(COMPLETE_MARKER).is_file()
        {
            reports.push(path);
        }
    }
    Ok(reports)
}

fn merge_function_types(
    merged: &mut BTreeMap<String, Vec<String>>,
    incoming: std::collections::HashMap<String, Vec<String>>,
) -> Result<(), AssembleError> {
    for (name, types) in incoming {
        if let Some(previous) = merged.get(&name)
            && previous != &types
        {
            return Err(AssembleError::ConflictingFunction {
                name,
                first: previous.join(", "),
                second: types.join(", "),
            });
        }
        merged.insert(name, types);
    }
    Ok(())
}

fn merge_events(
    merged: &mut BTreeMap<String, Vec<String>>,
    incoming: Vec<(String, Vec<String>)>,
) -> Result<(), AssembleError> {
    for (name, types) in incoming {
        if let Some(previous) = merged.get(&name)
            && previous != &types
        {
            return Err(AssembleError::ConflictingEvent {
                name,
                first: previous.join(", "),
                second: types.join(", "),
            });
        }
        merged.insert(name, types);
    }
    Ok(())
}

fn strip_meta_header(content: &str) -> String {
    let mut lines = content.lines();
    if lines.next().is_some_and(|line| line.trim() == "---@meta") {
        lines
            .skip_while(|line| line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    } else {
        content.trim().to_owned()
    }
}

fn resolve_function_lookups(
    content: &str,
    function_types: &BTreeMap<String, Vec<String>>,
) -> String {
    let mut resolved = content.to_owned();
    for (name, returns) in function_types {
        let replacement = returns.join(", ");
        resolved = resolved.replace(&format!("@lookup:{name}"), &replacement);
        if let Some(short_name) = name.rsplit('.').next() {
            resolved = resolved.replace(&format!("@lookup:{short_name}"), &replacement);
        }
    }
    resolved
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedSnapshot {
    pub path: PathBuf,
    pub unit_count: usize,
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("legacy generated Lua file `{}` would coexist with the single snapshot; remove the old output directory once", path.display())]
    LegacyLayout { path: PathBuf },
    #[error("failed to inspect output directory `{}`", path.display())]
    InspectOutput {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to create a temporary snapshot in `{}`", path.display())]
    CreateTemporary {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write the complete generated snapshot")]
    Write(#[source] io::Error),
    #[error("failed to atomically publish `{}`", path.display())]
    Persist {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn publish_snapshot(
    output_dir: &Path,
    snapshot: &GeneratedSnapshot,
) -> Result<PublishedSnapshot, PublishError> {
    let destination = output_dir.join(SNAPSHOT_FILE);
    if let Some(path) = find_legacy_lua_file(output_dir, &destination)? {
        return Err(PublishError::LegacyLayout { path });
    }

    let mut temporary = tempfile::NamedTempFile::new_in(output_dir).map_err(|source| {
        PublishError::CreateTemporary {
            path: output_dir.to_owned(),
            source,
        }
    })?;
    temporary
        .write_all(snapshot.content.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(PublishError::Write)?;
    temporary
        .persist(&destination)
        .map_err(|error| PublishError::Persist {
            path: destination.clone(),
            source: error.error,
        })?;

    Ok(PublishedSnapshot {
        path: destination,
        unit_count: snapshot.unit_count,
    })
}

fn find_legacy_lua_file(
    output_dir: &Path,
    destination: &Path,
) -> Result<Option<PathBuf>, PublishError> {
    let entries = std::fs::read_dir(output_dir).map_err(|source| PublishError::InspectOutput {
        path: output_dir.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| PublishError::InspectOutput {
            path: output_dir.to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| PublishError::InspectOutput {
                path: path.clone(),
                source,
            })?;
        if file_type.is_dir() {
            if let Some(legacy) = find_legacy_lua_file(&path, destination)? {
                return Ok(Some(legacy));
            }
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("lua")
            && path != destination
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
