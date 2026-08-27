use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

#[test]
fn cli_generates_stubs_for_a_stable_project() {
    let fixture = prepare_fixture();
    let output_dir = fixture.join("lua-types");

    let output = run_typegen(&fixture, &output_dir);

    assert!(
        output.status.success(),
        "mlua-typegen failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let snapshot_path = output_dir.join("mlua-typegen.lua");
    let stub = fs::read_to_string(&snapshot_path).expect("generated stub should exist");

    assert!(stub.contains("---@class (exact) Vec2"), "{stub}");
    assert!(stub.contains("---@param other_x number"), "{stub}");
    assert!(stub.contains("---@param other_y number"), "{stub}");
    assert!(stub.contains("---@return number"), "{stub}");
    assert_eq!(
        stub.matches("---@class (exact) NestedValue").count(),
        1,
        "{stub}"
    );
    assert!(
        stub.contains("---@alias EventName \"nested-ready\""),
        "{stub}"
    );
    assert!(stub.contains("---@param input string"), "{stub}");
    assert!(stub.contains("---@param id string"), "{stub}");
    assert!(stub.contains("---@return integer"), "{stub}");
    assert!(
        stub.contains("_G[\"convert_id\"] = function(id) end"),
        "{stub}"
    );
    assert!(
        stub.contains(
            "---@param mode string\n---@return \"fast-path\" | \"slow-path\"\n_G[\"echo_mode\"] = function(mode) end"
        ),
        "{stub}"
    );
    assert!(
        stub.contains(
            "---@param config { count: integer, enabled: boolean }\n---@return { count: integer, enabled: boolean }\n_G[\"roundtrip_config\"] = function(config) end"
        ),
        "{stub}"
    );
    assert!(stub.contains("---@class (exact) RemoteThing"), "{stub}");
    assert!(stub.contains("---@class (exact) RegistryThing"), "{stub}");
    assert!(
        stub.contains("RemoteThing[\"value\"] = function(self) end"),
        "{stub}"
    );
    assert!(
        stub.contains(
            "---@param thing RemoteThing\n---@return RemoteThing\n_G[\"roundtrip_remote\"] = function(thing) end"
        ),
        "{stub}"
    );
    assert!(
        stub.contains(
            "---@param thing RegistryThing\n---@return RegistryThing\n_G[\"roundtrip_registry\"] = function(thing) end"
        ),
        "{stub}"
    );
    assert!(
        stub.contains("Vec2[\"dot\"] = function(self, other_x, other_y) end"),
        "{stub}"
    );
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostics.contains("rust_type: \"Mode\"")
            && diagnostics.contains("String(\"fast-path\")")
            && diagnostics.contains("String(\"slow-path\")"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains("rust_type: \"Config\"")
            && diagnostics.contains("name: \"enabled\"")
            && diagnostics.contains("name: \"count\""),
        "{diagnostics}"
    );
    assert!(diagnostics.contains("openness: Open"), "{diagnostics}");
    assert!(
        diagnostics.contains("warning: explicit mapping `unused-mapping` was not used"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains("rust_type: \"Level\"")
            && diagnostics.contains("Integer(7)")
            && diagnostics.contains("Integer(11)"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains("rust_type: \"DynamicLabel\"")
            && diagnostics.contains("into_lua: Some(String)"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains("rust_type: \"DerivedValue\"")
            && diagnostics.contains("from_lua: Some(UserData(\"DerivedValue\"))"),
        "{diagnostics}"
    );
    validate_external_dialects(&fixture, &output_dir);

    fs::remove_file(&snapshot_path).unwrap();
    let rerun = run_typegen(&fixture, &output_dir);
    assert!(
        rerun.status.success(),
        "a warm Cargo project must still regenerate its snapshot\n{}",
        String::from_utf8_lossy(&rerun.stderr)
    );
    let known_good = fs::read_to_string(&snapshot_path).unwrap();

    let config_path = fixture.join("mlua-typegen.toml");
    let valid_config = fs::read_to_string(&config_path).unwrap();
    let invalid_generics = valid_config.replace(
        "params = [\"T\"]\nfrom_lua = \"string\"\ninto_lua = \"$T\"",
        "params = [\"U\"]\nfrom_lua = \"string\"\ninto_lua = \"$U\"",
    );
    fs::write(&config_path, invalid_generics).unwrap();
    let invalid_mapping = run_typegen(&fixture, &output_dir);
    assert!(!invalid_mapping.status.success());
    let invalid_mapping_stderr = String::from_utf8_lossy(&invalid_mapping.stderr);
    assert!(
        invalid_mapping_stderr.contains("[MLTG1003]")
            && invalid_mapping_stderr.contains("rustc resolved [\"T\"]"),
        "{invalid_mapping_stderr}"
    );
    assert_eq!(fs::read_to_string(&snapshot_path).unwrap(), known_good);
    fs::write(&config_path, valid_config).unwrap();

    let missing_direction = fs::read_to_string(&config_path)
        .unwrap()
        .replace("from_lua = \"string\"\n", "");
    fs::write(&config_path, missing_direction).unwrap();
    let unresolved = run_typegen(&fixture, &output_dir);
    assert!(!unresolved.status.success());
    let unresolved_stderr = String::from_utf8_lossy(&unresolved.stderr);
    assert!(
        unresolved_stderr.contains("[MLTG1002]")
            && unresolved_stderr.contains("Lua-visible path: `convert_id`")
            && unresolved_stderr.contains("mapping `wire-id` does not declare FromLua"),
        "{unresolved_stderr}"
    );
    assert_eq!(fs::read_to_string(&snapshot_path).unwrap(), known_good);
    let valid_config = fs::read_to_string(&config_path).unwrap().replace(
        "into_lua = \"$T\"",
        "from_lua = \"string\"\ninto_lua = \"$T\"",
    );
    fs::write(&config_path, valid_config).unwrap();

    let source_path = fixture.join("src/lib.rs");
    let source = fs::read_to_string(&source_path).unwrap();
    fs::write(
        &source_path,
        format!(
            "{source}\n\npub struct OpaqueAlpha;\n\nimpl IntoLua for OpaqueAlpha {{\n    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {{\n        lua.globals().get::<Value>(\"opaque-alpha\")\n    }}\n}}\n\npub struct OpaqueZeta;\n\nimpl IntoLua for OpaqueZeta {{\n    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {{\n        lua.globals().get::<Value>(\"opaque-zeta\")\n    }}\n}}\n"
        ),
    )
    .unwrap();
    let incomplete = run_typegen(&fixture, &output_dir);
    assert!(!incomplete.status.success());
    let incomplete_stderr = String::from_utf8_lossy(&incomplete.stderr);
    assert_eq!(
        incomplete_stderr.matches("[MLTG1001]").count(),
        2,
        "{incomplete_stderr}"
    );
    assert!(
        incomplete_stderr.contains("stage: extraction"),
        "{incomplete_stderr}"
    );
    assert!(
        incomplete_stderr.contains("Rust owner:"),
        "{incomplete_stderr}"
    );
    assert!(incomplete_stderr.contains("reason:"), "{incomplete_stderr}");
    assert!(incomplete_stderr.contains("help:"), "{incomplete_stderr}");
    let alpha = incomplete_stderr.find("OpaqueAlpha").unwrap();
    let zeta = incomplete_stderr.find("OpaqueZeta").unwrap();
    assert!(alpha < zeta, "{incomplete_stderr}");
    assert!(
        incomplete_stderr.contains(&format!(
            "preserved previous snapshot at {}",
            snapshot_path.display()
        )),
        "{incomplete_stderr}"
    );
    assert_eq!(fs::read_to_string(&snapshot_path).unwrap(), known_good);

    fs::write(fixture.join("src/lib.rs"), "this is not valid Rust").unwrap();
    let failed = run_typegen(&fixture, &output_dir);
    assert!(!failed.status.success());
    assert_eq!(fs::read_to_string(snapshot_path).unwrap(), known_good);
}

fn run_typegen(fixture: &Path, output_dir: &Path) -> std::process::Output {
    run_typegen_with(fixture, output_dir, &[])
}

fn run_typegen_with(fixture: &Path, output_dir: &Path, options: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mlua-typegen"))
        .arg("--output")
        .arg(output_dir)
        .args(options)
        .env("CARGO_TERM_COLOR", "never")
        .env("MLUA_TYPEGEN_TRACE", "1")
        .current_dir(fixture)
        .output()
        .expect("failed to run mlua-typegen")
}

fn validate_external_dialects(fixture: &Path, lua_ls_output: &Path) {
    if let Some(lua_ls) = std::env::var_os("MLUA_TYPEGEN_LUALS") {
        let output = Command::new(lua_ls)
            .arg(format!("--check={}", lua_ls_output.display()))
            .arg("--checklevel=Warning")
            .arg("--check_format=pretty")
            .arg(format!("--logpath={}", fixture.join("luals-log").display()))
            .output()
            .expect("failed to run LuaLS validator");
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .contains("Diagnosis completed, no problems found"),
            "LuaLS rejected generated output\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    if let Some(emmylua) = std::env::var_os("MLUA_TYPEGEN_EMMYLUA_CHECK") {
        let output_dir = fixture.join("emmylua-types");
        let generated = run_typegen_with(fixture, &output_dir, &["--emmylua"]);
        assert!(
            generated.status.success(),
            "EmmyLua generation failed: {}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let output = Command::new(emmylua)
            .arg("--warnings-as-errors")
            .arg(&output_dir)
            .output()
            .expect("failed to run EmmyLua validator");
        assert!(
            output.status.success(),
            "EmmyLua rejected generated output\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn prepare_fixture() -> PathBuf {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("cli-smoke")
        .join(std::process::id().to_string());

    let _ = fs::remove_dir_all(&fixture);
    fs::create_dir_all(fixture.join("src")).expect("failed to create smoke fixture");
    fs::create_dir_all(fixture.join("wire-domain/src"))
        .expect("failed to create path dependency fixture");
    install_local_registry(&fixture);

    fs::write(
        fixture.join("Cargo.toml"),
        r#"[package]
name = "mlua-typegen-cli-smoke"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
mlua = { version = "=0.12.0", features = ["lua54", "vendored", "macros"] }
wire-domain = { path = "wire-domain" }
registry-domain = { version = "=0.1.0", registry = "fixture" }
"#,
    )
    .expect("failed to write fixture Cargo.toml");

    fs::write(
        fixture.join("wire-domain/Cargo.toml"),
        r#"[package]
name = "wire-domain"
version = "0.1.0"
edition = "2024"

[dependencies]
mlua = { version = "=0.12.0", features = ["lua54", "vendored"] }
"#,
    )
    .expect("failed to write path dependency Cargo.toml");
    fs::write(
        fixture.join("wire-domain/src/lib.rs"),
        r#"use mlua::{FromLua, IntoLua, Lua, UserData, UserDataMethods, Value};

pub struct WireId<T>(pub T);

impl FromLua for WireId<i64> {
    fn from_lua(value: Value, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            Value::String(value) => Ok(Self(value.to_str()?.parse().map_err(mlua::Error::external)?)),
            _ => Err(mlua::Error::runtime("expected wire id")),
        }
    }
}

impl IntoLua for WireId<i64> {
    fn into_lua(self, _lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::Integer(self.0))
    }
}

#[derive(Clone, Copy)]
pub struct RemoteThing(pub i64);

impl UserData for RemoteThing {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("value", |_, this, ()| Ok(this.0));
    }
}

impl FromLua for RemoteThing {
    fn from_lua(value: Value, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            Value::UserData(value) => Ok(*value.borrow::<Self>()?),
            _ => Err(mlua::Error::runtime("expected RemoteThing userdata")),
        }
    }
}
"#,
    )
    .expect("failed to write path dependency source");

    fs::write(
        fixture.join("src").join("lib.rs"),
        r#"use mlua::{FromLua, IntoLua, Lua, UserData, UserDataMethods, Value};
use registry_domain::RegistryThing;
use wire_domain::{RemoteThing, WireId};

pub struct UnusedMapping<T>(pub T);

pub enum Mode {
    Fast,
    Slow,
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Fast => "fast-path",
        Mode::Slow => "slow-path",
    }
}

impl IntoLua for Mode {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::String(lua.create_string(mode_name(self))?))
    }
}

pub enum Level {
    Low,
    High,
}

fn level_value(level: Level) -> Value {
    Value::Integer(match level {
        Level::Low => 7,
        Level::High => 11,
    })
}

impl IntoLua for Level {
    fn into_lua(self, _lua: &Lua) -> mlua::Result<Value> {
        Ok(level_value(self))
    }
}

impl FromLua for Level {
    fn from_lua(value: Value, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            Value::Integer(7) => Ok(Self::Low),
            Value::Integer(11) => Ok(Self::High),
            _ => Err(mlua::Error::runtime("unsupported level")),
        }
    }
}

pub struct DynamicLabel(pub i64);

impl IntoLua for DynamicLabel {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        Ok(Value::String(lua.create_string(self.0.to_string())?))
    }
}

#[derive(Clone, Copy, mlua::FromLua)]
pub struct DerivedValue;

impl FromLua for Mode {
    fn from_lua(value: Value, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            Value::String(value) if value.to_str()? == "fast-path" => Ok(Self::Fast),
            Value::String(value) if value.to_str()? == "slow-path" => Ok(Self::Slow),
            _ => Err(mlua::Error::runtime("unsupported mode")),
        }
    }
}

pub struct Config {
    pub enabled: bool,
    pub count: i64,
}

impl IntoLua for Config {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        let table = lua.create_table()?;
        table.set("enabled", self.enabled)?;
        table.set("count", self.count)?;
        Ok(Value::Table(table))
    }
}

impl FromLua for Config {
    fn from_lua(value: Value, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            Value::Table(table) => Ok(Self {
                enabled: table.get("enabled")?,
                count: table.get("count")?,
            }),
            _ => Err(mlua::Error::runtime("expected config table")),
        }
    }
}

pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl UserData for Vec2 {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("dot", |_, this, (other_x, other_y): (f64, f64)| {
            Ok(this.x * other_x + this.y * other_y)
        });
    }
}

mod nested {
    use mlua::{UserData, UserDataMethods};

    pub struct NestedValue(pub i64);

    impl UserData for NestedValue {
        fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
            methods.add_method("value", |_, this, ()| Ok(this.0));
        }
    }
}

fn emit_event(_lua: &Lua, _event: (String, mlua::MultiValue)) {}

pub fn register(lua: &Lua) -> mlua::Result<()> {
    let args = lua.pack_multi((true, 42_i64))?;
    emit_event(lua, ("nested-ready".to_string(), args));
    lua.globals().set(
        "mixed_fn",
        lua.create_function(|_, input: String| Ok(input.len() as i64))?,
    )?;
    lua.globals().set(
        "convert_id",
        lua.create_function(|_, id: WireId<i64>| Ok(id))?,
    )?;
    lua.globals().set(
        "echo_mode",
        lua.create_function(|_, mode: Mode| Ok(mode))?,
    )?;
    lua.globals().set(
        "roundtrip_config",
        lua.create_function(|_, config: Config| Ok(config))?,
    )?;
    lua.globals().set(
        "roundtrip_remote",
        lua.create_function(|_, thing: RemoteThing| Ok(thing))?,
    )?;
    lua.globals().set(
        "roundtrip_registry",
        lua.create_function(|_, thing: RegistryThing| Ok(thing))?,
    )?;
    Ok(())
}
"#,
    )
    .expect("failed to write fixture lib.rs");

    fs::write(
        fixture.join("mlua-typegen.toml"),
        r#"#:schema https://raw.githubusercontent.com/themixednuts/mlua-typegen/main/schemas/mlua-typegen.schema.json

[[mappings]]
id = "wire-id"
rust = { package = "wire-domain", path = "WireId" }
params = ["T"]
from_lua = "string"
into_lua = "$T"

[[mappings]]
id = "unused-mapping"
rust = { package = "mlua-typegen-cli-smoke", path = "UnusedMapping" }
params = ["T"]
lua = "string"
"#,
    )
    .expect("failed to write fixture mapping config");

    fixture
}

fn install_local_registry(fixture: &Path) {
    let source = fixture.join("registry-domain-source");
    fs::create_dir_all(source.join("src")).unwrap();
    fs::write(
        source.join("Cargo.toml"),
        r#"[package]
name = "registry-domain"
version = "0.1.0"
edition = "2024"

[dependencies]
mlua = { version = "=0.12.0", features = ["lua54", "vendored"] }
"#,
    )
    .unwrap();
    fs::write(
        source.join("src/lib.rs"),
        r#"use mlua::{FromLua, Lua, UserData, UserDataMethods, Value};

#[derive(Clone, Copy)]
pub struct RegistryThing(pub i64);

impl UserData for RegistryThing {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("value", |_, this, ()| Ok(this.0));
    }
}

impl FromLua for RegistryThing {
    fn from_lua(value: Value, _lua: &Lua) -> mlua::Result<Self> {
        match value {
            Value::UserData(value) => Ok(*value.borrow::<Self>()?),
            _ => Err(mlua::Error::runtime("expected RegistryThing userdata")),
        }
    }
}
"#,
    )
    .unwrap();

    let package = Command::new("cargo")
        .args(["package", "--allow-dirty", "--no-verify"])
        .env("CARGO_TARGET_DIR", source.join("target"))
        .current_dir(&source)
        .output()
        .expect("failed to package registry fixture");
    assert!(
        package.status.success(),
        "registry fixture packaging failed: {}",
        String::from_utf8_lossy(&package.stderr)
    );
    let archive = source.join("target/package/registry-domain-0.1.0.crate");
    let archive_bytes = fs::read(&archive).unwrap();
    let checksum = Sha256::digest(&archive_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    let download_root = fixture.join("registry-download");
    let download = download_root.join("registry-domain/0.1.0/download");
    fs::create_dir_all(download.parent().unwrap()).unwrap();
    fs::write(&download, archive_bytes).unwrap();

    let index = fixture.join("registry-index");
    fs::create_dir_all(index.join("re/gi")).unwrap();
    fs::write(
        index.join("config.json"),
        format!(r#"{{"dl":"{}"}}"#, file_url(&download_root)),
    )
    .unwrap();
    let entry = serde_json::json!({
        "name": "registry-domain",
        "vers": "0.1.0",
        "deps": [{
            "name": "mlua",
            "req": "=0.12.0",
            "features": ["lua54", "vendored"],
            "optional": false,
            "default_features": true,
            "target": null,
            "kind": "normal",
            "registry": "https://github.com/rust-lang/crates.io-index",
            "package": null
        }],
        "cksum": checksum,
        "features": {},
        "yanked": false,
        "links": null,
        "v": 2
    });
    fs::write(index.join("re/gi/registry-domain"), format!("{entry}\n")).unwrap();

    for args in [
        &["init", "--quiet"][..],
        &["config", "user.email", "fixture@example.invalid"][..],
        &["config", "user.name", "Fixture"][..],
        &["add", "."][..],
        &["commit", "--quiet", "-m", "fixture index"][..],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(&index)
            .status()
            .expect("failed to construct local registry index");
        assert!(status.success(), "git command failed: {args:?}");
    }

    fs::create_dir_all(fixture.join(".cargo")).unwrap();
    fs::write(
        fixture.join(".cargo/config.toml"),
        format!("[registries.fixture]\nindex = {:?}\n", file_url(&index)),
    )
    .unwrap();
}

fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}
