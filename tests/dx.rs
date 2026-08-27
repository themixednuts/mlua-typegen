use std::process::Command;

#[test]
fn cargo_subcommand_prints_help_and_version_without_a_workspace() {
    let executable = env!("CARGO_BIN_EXE_cargo-mlua-typegen");

    let help = Command::new(executable)
        .args(["mlua-typegen", "--help"])
        .output()
        .expect("failed to run Cargo-style help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help output is UTF-8");
    assert!(help.contains("Usage: cargo mlua-typegen"), "{help}");
    assert!(help.contains("--output <DIRECTORY>"), "{help}");

    let version = Command::new(executable)
        .args(["mlua-typegen", "--version"])
        .output()
        .expect("failed to run Cargo-style version");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("mlua-typegen {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn installers_pin_the_repository_toolchain_and_install_the_cargo_plugin() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml")).unwrap();
    let channel = toolchain
        .lines()
        .find_map(|line| line.strip_prefix("channel = \"")?.strip_suffix('"'))
        .expect("rust-toolchain.toml declares a channel");

    for path in ["scripts/install.sh", "scripts/install.ps1"] {
        let installer = std::fs::read_to_string(root.join(path)).unwrap();
        assert!(installer.contains(channel), "{path} must use {channel}");
        for component in ["rustc-dev", "llvm-tools-preview"] {
            assert!(
                installer.contains(component),
                "{path} must install {component}"
            );
        }
        assert!(
            installer.contains("cargo mlua-typegen --version"),
            "{path} must verify the installed Cargo plugin"
        );
    }
}
