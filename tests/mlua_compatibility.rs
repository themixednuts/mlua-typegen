use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_metadata::MetadataCommand;
use semver::{Version, VersionReq};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityMatrix {
    supported: String,
    lanes: Vec<CompatibilityLane>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityLane {
    name: String,
    fixture: String,
    requirement: String,
    locked: Version,
    expected: String,
    updates: UpdatePolicy,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum UpdatePolicy {
    Minimum,
    LatestPatch,
    Boundary,
    MinimumAndLatest,
}

#[test]
fn matrix_declares_the_crate_range_once_and_has_valid_lanes() {
    let matrix = load_matrix();
    assert_eq!(matrix.supported, mlua_typegen::SUPPORTED_MLUA_VERSIONS);

    let supported = VersionReq::parse(&matrix.supported).unwrap();
    let mut names = BTreeSet::new();
    let mut fixtures = BTreeSet::new();
    for lane in &matrix.lanes {
        assert!(names.insert(&lane.name), "duplicate lane `{}`", lane.name);
        assert!(
            fixtures.insert(&lane.fixture),
            "duplicate fixture `{}`",
            lane.fixture
        );
        let requirement = VersionReq::parse(&lane.requirement).unwrap();
        assert!(requirement.matches(&lane.locked), "lane `{}`", lane.name);
        assert!(supported.matches(&lane.locked), "lane `{}`", lane.name);
        assert!(
            fixture_root()
                .join(&lane.fixture)
                .join("Cargo.lock")
                .is_file()
        );
        assert!(fixture_root().join(&lane.expected).is_file());
        assert_manifest_requirement(lane);
    }

    assert!(matrix.lanes.iter().any(|lane| {
        lane.locked == Version::new(0, 11, 4) && matches!(lane.updates, UpdatePolicy::Boundary)
    }));
    assert!(matrix.lanes.iter().any(|lane| {
        lane.locked == Version::new(0, 11, 5) && matches!(lane.updates, UpdatePolicy::Boundary)
    }));
}

#[test]
#[ignore = "runs the real rustc driver over every locked upstream mlua fixture"]
fn locked_real_mlua_matrix_generates_reviewed_semantics() {
    let matrix = load_matrix();
    let selected = std::env::var("MLUA_TYPEGEN_MATRIX_LANE").ok();
    let lanes = matrix
        .lanes
        .iter()
        .filter(|lane| selected.as_ref().is_none_or(|name| name == &lane.name))
        .collect::<Vec<_>>();
    assert!(
        !lanes.is_empty(),
        "selected compatibility lane does not exist"
    );

    for lane in lanes {
        verify_lane(lane);
    }
}

fn verify_lane(lane: &CompatibilityLane) {
    let manifest = fixture_root().join(&lane.fixture).join("Cargo.toml");
    let mut metadata = MetadataCommand::new();
    metadata
        .manifest_path(&manifest)
        .other_options(vec!["--locked".to_owned()]);
    let metadata = metadata
        .exec()
        .unwrap_or_else(|error| panic!("lane `{}` metadata failed: {error}", lane.name));
    let resolved = metadata
        .packages
        .iter()
        .find(|package| package.name.as_str() == "mlua")
        .unwrap_or_else(|| panic!("lane `{}` resolved no mlua package", lane.name));
    assert_eq!(resolved.version, lane.locked, "lane `{}`", lane.name);

    let expected = std::fs::read_to_string(fixture_root().join(&lane.expected)).unwrap();
    for dialect in [None, Some("--emmylua")] {
        let output_root = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_mlua-typegen"));
        command
            .args(["--output"])
            .arg(output_root.path())
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--locked")
            .env("MLUA_TYPEGEN_TRACE", "1");
        if let Some(flag) = dialect {
            command.arg(flag);
        }
        let output = command
            .output()
            .unwrap_or_else(|error| panic!("lane `{}` failed to start: {error}", lane.name));
        assert!(
            output.status.success(),
            "lane `{}` failed\nstdout:\n{}\nstderr:\n{}",
            lane.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let actual = std::fs::read_to_string(output_root.path().join("mlua-typegen.lua"))
            .unwrap_or_else(|error| panic!("lane `{}` produced no snapshot: {error}", lane.name));
        assert_eq!(actual, expected, "lane `{}` output changed", lane.name);

        if lane.expected != "expected/generated.lua" {
            let trace = String::from_utf8_lossy(&output.stderr);
            assert!(trace.contains("rust_type: \"WireMode\""), "{trace}");
            assert!(
                trace.contains(
                    "into_lua: Some(Union([Literal(String(\"fast\")), Literal(String(\"slow\"))]))"
                ),
                "{trace}"
            );
            assert!(trace.contains("from_lua: Some(String)"), "{trace}");
        }
    }
}

fn assert_manifest_requirement(lane: &CompatibilityLane) {
    let manifest = fixture_root().join(&lane.fixture).join("Cargo.toml");
    let document = std::fs::read_to_string(&manifest).unwrap();
    let document = toml::from_str::<toml::Value>(&document).unwrap();
    let requirement = document["dependencies"]["mlua"]["version"]
        .as_str()
        .unwrap();
    assert_eq!(requirement, lane.requirement, "lane `{}`", lane.name);
}

fn load_matrix() -> CompatibilityMatrix {
    let source = std::fs::read_to_string(fixture_root().join("matrix.toml")).unwrap();
    toml::from_str(&source).unwrap()
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mlua")
}
