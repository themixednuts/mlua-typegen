use std::fs;
use std::process::Command;

#[test]
fn explicit_config_is_rejected_before_cargo_metadata_or_compilation() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("broken.toml");
    fs::write(
        &config,
        r#"
[[mappings]]
id = "unsafe-fallback"
rust = { package = "missing", path = "Thing" }
lua = "any"
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_mlua-typegen"))
        .args(["--config", config.to_str().unwrap()])
        .current_dir(root.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bare `any` is forbidden"), "{stderr}");
    assert!(
        !stderr.contains("failed to query Cargo workspace metadata"),
        "{stderr}"
    );
}
