//! Independent annotation-dialect adapters for the canonical semantic API.

mod dialect;
mod render;

use std::fmt::Write as _;
use std::path::Path;

use crate::{Dialect, EventEmission, ExtractedUnit, SemanticApi};

pub use dialect::{DialectFeature, RenderError, RenderedDocument};

/// Render one immutable semantic API through a closed dialect adapter.
pub fn render(api: &SemanticApi, dialect: Dialect) -> Result<RenderedDocument, RenderError> {
    dialect::render(api, dialect)
}

pub(crate) fn write_stubs_for(
    output_dir: &Path,
    unit: &ExtractedUnit,
    dialect: Dialect,
) -> Result<(), WriteError> {
    std::fs::create_dir_all(output_dir)?;
    let document = render(&unit.api, dialect)?;
    std::fs::write(output_dir.join(ANNOTATIONS_FILE), document.as_str())?;
    append_function_types(output_dir, &unit.api)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Render(#[from] RenderError),
}

const ANNOTATIONS_FILE: &str = "annotations.lua";
const EVENTS_FILE: &str = ".events.txt";
const FUNC_TYPES_FILE: &str = ".func_types.txt";

pub(crate) fn append_event_emissions(
    output_dir: &Path,
    emissions: &[EventEmission],
) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::create_dir_all(output_dir)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output_dir.join(EVENTS_FILE))?;
    for emission in emissions {
        let types = emission
            .arg_types
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        writeln!(file, "{}\t{}", emission.event_name, types.join("\t"))?;
    }
    Ok(())
}

fn append_function_types(output_dir: &Path, api: &SemanticApi) -> std::io::Result<()> {
    use crate::ValueDef;
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output_dir.join(FUNC_TYPES_FILE))?;
    for (path, value) in api.values() {
        let ValueDef::Table(table) = value else {
            continue;
        };
        for (name, overloads) in &table.functions {
            let returns = overloads
                .primary()
                .outputs
                .iter()
                .map(|value| render::type_expr(&value.ty))
                .collect::<Vec<_>>();
            if !returns.is_empty()
                && let Some(key) = function_lookup_key(path, name)
            {
                writeln!(file, "{key}\t{}", returns.join("\t"))?;
            }
        }
    }
    Ok(())
}

fn function_lookup_key(path: &crate::LuaPath, name: &crate::LuaName) -> Option<String> {
    let [module] = path.segments() else {
        return None;
    };
    Some(format!(
        "{}.{}",
        std::str::from_utf8(module.as_bytes()).ok()?,
        std::str::from_utf8(name.as_bytes()).ok()?
    ))
}

pub(crate) fn try_read_function_types(
    output_dir: &Path,
) -> std::io::Result<std::collections::HashMap<String, Vec<String>>> {
    read_unique_rows(output_dir.join(FUNC_TYPES_FILE), "return types")
}

pub(crate) fn try_read_event_emissions(
    output_dir: &Path,
) -> std::io::Result<Vec<(String, Vec<String>)>> {
    Ok(
        read_unique_rows(output_dir.join(EVENTS_FILE), "callback types")?
            .into_iter()
            .collect(),
    )
}

fn read_unique_rows(
    path: std::path::PathBuf,
    description: &str,
) -> std::io::Result<std::collections::HashMap<String, Vec<String>>> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(error),
    };
    let mut rows = std::collections::HashMap::<String, Vec<String>>::new();
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let mut cells = line.trim().split('\t');
        let key = cells.next().expect("a non-empty row has a key").to_owned();
        let values = cells
            .filter(|cell| !cell.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Some(previous) = rows.get(&key)
            && previous != &values
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "conflicting {description} for `{key}`: `{}` and `{}`",
                    previous.join(", "),
                    values.join(", ")
                ),
            ));
        }
        rows.insert(key, values);
    }
    Ok(rows)
}

#[must_use]
pub(crate) fn generate_event_stubs(events: &[(String, Vec<String>)]) -> String {
    let mut out = String::from("---@meta\n\n");
    let names = events
        .iter()
        .map(|(name, _)| format!("{name:?}"))
        .collect::<Vec<_>>();
    writeln!(out, "---@alias EventName {}\n", names.join(" | ")).unwrap();
    for (name, types) in events {
        let id = name
            .chars()
            .map(|ch| if ch.is_alphanumeric() { ch } else { '_' })
            .collect::<String>();
        let params = types
            .iter()
            .enumerate()
            .map(|(index, ty)| format!("p{}: {ty}", index + 1))
            .collect::<Vec<_>>();
        writeln!(
            out,
            "---@alias EventCallback_{id} fun({})",
            params.join(", ")
        )
        .unwrap();
    }
    out
}

#[cfg(test)]
mod tests;
