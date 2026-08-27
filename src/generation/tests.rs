use std::fs;

use super::*;

fn complete_report(root: &Path, name: &str, lua: &str) -> PathBuf {
    let report = root.join(name);
    fs::create_dir_all(&report).unwrap();
    fs::write(report.join(format!("{name}.lua")), lua).unwrap();
    fs::write(report.join(MAPPING_USAGE_FILE), "[]").unwrap();
    fs::write(
        report.join(UNIT_MANIFEST_FILE),
        serde_json::to_vec(&UnitManifest {
            stable_crate_id: name.bytes().fold(0_u64, |hash, byte| {
                hash.wrapping_mul(1099511628211)
                    .wrapping_add(u64::from(byte))
            }),
            crate_name: name.to_owned(),
            generation: 0,
            scope: None,
            pending: false,
            requirements: BTreeSet::new(),
            provided_classes: BTreeMap::new(),
            conversions: Vec::new(),
        })
        .unwrap(),
    )
    .unwrap();
    fs::write(report.join(COMPLETE_MARKER), format!("{name}\n")).unwrap();
    report
}

#[test]
fn assembles_complete_reports_in_content_order() {
    let workspace = tempfile::tempdir().unwrap();
    let second = complete_report(
        workspace.path(),
        "zeta",
        "---@meta\n\n---@class Zeta\nZeta = {}\n",
    );
    let first = complete_report(
        workspace.path(),
        "alpha",
        "---@meta\n\n---@class Alpha\nAlpha = {}\n---@return @lookup:make\n",
    );
    fs::write(first.join(".func_types.txt"), "factory.make\tstring\n").unwrap();
    fs::write(second.join(".events.txt"), "ready\tinteger\n").unwrap();

    let snapshot = assemble_reports(workspace.path()).unwrap();

    assert_eq!(snapshot.unit_count(), 2);
    assert_eq!(snapshot.content().matches("---@meta").count(), 1);
    assert!(snapshot.content().contains("---@return string"));
    assert!(snapshot.content().contains("---@alias EventName \"ready\""));
    assert!(snapshot.content().find("---@class Alpha") < snapshot.content().find("---@class Zeta"));
}

#[test]
fn unions_mapping_usage_from_complete_reports_only() {
    let workspace = tempfile::tempdir().unwrap();
    let first = complete_report(workspace.path(), "first", "---@meta\n");
    let second = complete_report(workspace.path(), "second", "---@meta\n");
    fs::write(first.join(MAPPING_USAGE_FILE), r#"["alpha","shared"]"#).unwrap();
    fs::write(second.join(MAPPING_USAGE_FILE), r#"["shared","zeta"]"#).unwrap();
    let partial = workspace.path().join("partial");
    fs::create_dir(&partial).unwrap();
    fs::write(partial.join(MAPPING_USAGE_FILE), r#"["ignored"]"#).unwrap();

    let snapshot = assemble_reports(workspace.path()).unwrap();
    let ids = snapshot
        .used_mappings()
        .iter()
        .map(MappingId::as_str)
        .collect::<Vec<_>>();

    assert_eq!(ids, ["alpha", "shared", "zeta"]);
}

#[test]
fn ignores_report_without_completion_marker() {
    let workspace = tempfile::tempdir().unwrap();
    complete_report(
        workspace.path(),
        "complete",
        "---@meta\n\n---@class Complete\nComplete = {}\n",
    );
    let partial = workspace.path().join("partial");
    fs::create_dir(&partial).unwrap();
    fs::write(
        partial.join("partial.lua"),
        "---@meta\n\n---@class Partial\nPartial = {}\n",
    )
    .unwrap();

    let snapshot = assemble_reports(workspace.path()).unwrap();

    assert!(snapshot.content().contains("Complete"));
    assert!(!snapshot.content().contains("Partial"));
}

#[test]
fn reachable_dependency_without_a_recorded_cargo_unit_is_an_explicit_error() {
    let workspace = RunWorkspace::create().unwrap();
    let report = complete_report(workspace.report_dir(), "consumer", "---@meta\n");
    let manifest_path = report.join(UNIT_MANIFEST_FILE);
    let mut manifest: UnitManifest =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest.pending = true;
    manifest
        .requirements
        .insert(crate::driver_protocol::RequiredEvidence {
            stable_crate_id: 0xdead_beef,
            rust_path: "missing_source::Thing".into(),
            lua_path: "api.consume".into(),
            direction: crate::driver_protocol::EvidenceDirection::FromLua,
        });
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

    let error = resolve_reachable_reports(
        &workspace,
        Path::new("unused-driver"),
        Path::new("unused-config"),
        Dialect::LuaLs,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ReachabilityError::SourceUnavailable {
            rust_path,
            lua_path,
            ..
        } if rust_path == "missing_source::Thing" && lua_path == "api.consume"
    ));
}

#[test]
fn rejects_conflicting_function_facts() {
    let workspace = tempfile::tempdir().unwrap();
    let first = complete_report(workspace.path(), "first", "---@meta\n");
    let second = complete_report(workspace.path(), "second", "---@meta\n");
    fs::write(first.join(".func_types.txt"), "api.call\tstring\n").unwrap();
    fs::write(second.join(".func_types.txt"), "api.call\tinteger\n").unwrap();

    let error = assemble_reports(workspace.path()).unwrap_err();

    assert!(matches!(error, AssembleError::ConflictingFunction { .. }));
}

#[test]
fn atomically_replaces_the_previous_snapshot() {
    let output = tempfile::tempdir().unwrap();
    let destination = output.path().join(SNAPSHOT_FILE);
    fs::write(&destination, "old").unwrap();
    let snapshot = GeneratedSnapshot {
        content: "---@meta\n\nnew\n".to_owned(),
        unit_count: 3,
        used_mappings: BTreeSet::new(),
    };

    let output_lock = OutputLock::acquire(output.path()).unwrap();
    let published = output_lock.publish(&snapshot).unwrap();

    assert_eq!(published.path, destination);
    assert_eq!(published.unit_count, 3);
    assert_eq!(
        fs::read_to_string(published.path).unwrap(),
        snapshot.content
    );
}

#[test]
fn publication_failure_preserves_the_previous_snapshot() {
    let output = tempfile::tempdir().unwrap();
    let destination = output.path().join(SNAPSHOT_FILE);
    fs::write(&destination, "known-good").unwrap();
    fs::write(output.path().join("legacy.lua"), "stale").unwrap();
    let snapshot = GeneratedSnapshot {
        content: "replacement".to_owned(),
        unit_count: 1,
        used_mappings: BTreeSet::new(),
    };

    let output_lock = OutputLock::acquire(output.path()).unwrap();
    let error = output_lock.publish(&snapshot).unwrap_err();

    assert!(matches!(error, PublishError::LegacyLayout { .. }));
    assert_eq!(fs::read_to_string(destination).unwrap(), "known-good");
}

#[test]
fn output_lock_rejects_a_concurrent_publisher() {
    let output = tempfile::tempdir().unwrap();
    let first = OutputLock::acquire(output.path()).unwrap();

    let error = OutputLock::acquire(output.path()).unwrap_err();

    assert!(matches!(error, LockError::AlreadyRunning { .. }));
    drop(first);
    OutputLock::acquire(output.path()).unwrap();
}
