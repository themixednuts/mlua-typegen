use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("schemas/mlua-typegen.schema.json"));
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&destination, mlua_typegen::config::schema_json()?)?;
    eprintln!("wrote {}", destination.display());
    Ok(())
}
