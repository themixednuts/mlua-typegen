use std::fmt;

/// One exact Lua table key used as a visible name.
///
/// Lua strings are byte sequences, so this type deliberately does not require
/// UTF-8. An empty name is valid.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LuaName(Box<[u8]>);

impl LuaName {
    pub fn from_bytes(bytes: impl Into<Box<[u8]>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<&str> for LuaName {
    fn from(value: &str) -> Self {
        Self::from_bytes(value.as_bytes())
    }
}

impl From<String> for LuaName {
    fn from(value: String) -> Self {
        Self::from_bytes(value.into_bytes())
    }
}

impl fmt::Debug for LuaName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "LuaName({})",
            lua_string_literal(self.as_bytes())
        )
    }
}

impl fmt::Display for LuaName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&diagnostic_name(self.as_bytes()))
    }
}

/// An exact, segmented path from the Lua global table to a registered value.
///
/// Segmentation is semantic: the single segment `a.b` is different from the
/// two segments `a`, `b`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LuaPath(Box<[LuaName]>);

impl LuaPath {
    pub fn global(name: impl Into<LuaName>) -> Self {
        Self(vec![name.into()].into_boxed_slice())
    }

    pub fn child(&self, name: impl Into<LuaName>) -> Self {
        let mut segments = self.0.to_vec();
        segments.push(name.into());
        Self(segments.into_boxed_slice())
    }

    pub fn segments(&self) -> &[LuaName] {
        &self.0
    }
}

impl fmt::Display for LuaPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, segment) in self.segments().iter().enumerate() {
            if index != 0 {
                formatter.write_str(" / ")?;
            }
            fmt::Display::fmt(segment, formatter)?;
        }
        Ok(())
    }
}

/// A Lua-source expression that accesses one exact [`LuaPath`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaAccess(String);

impl LuaAccess {
    pub fn for_path(path: &LuaPath) -> Self {
        let mut source = String::from("_G");
        for segment in path.segments() {
            source.push('[');
            source.push_str(&lua_string_literal(segment.as_bytes()));
            source.push(']');
        }
        Self(source)
    }

    pub fn child(&self, name: &LuaName) -> Self {
        let mut source = self.0.clone();
        source.push('[');
        source.push_str(&lua_string_literal(name.as_bytes()));
        source.push(']');
        Self(source)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LuaAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A Lua-source string literal for one exact [`LuaName`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaStringLiteral(String);

impl LuaStringLiteral {
    pub fn for_name(name: &LuaName) -> Self {
        Self(lua_string_literal(name.as_bytes()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LuaStringLiteral {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A reversible, collision-free annotation identifier for one [`LuaPath`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotationId(String);

impl AnnotationId {
    pub fn for_path(path: &LuaPath) -> Self {
        let mut id = String::from("__mlua");
        for segment in path.segments() {
            use fmt::Write as _;
            write!(id, "_{}x", segment.as_bytes().len()).unwrap();
            for byte in segment.as_bytes() {
                write!(id, "{byte:02x}").unwrap();
            }
        }
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AnnotationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn lua_string_literal(bytes: &[u8]) -> String {
    let mut literal = String::from("\"");
    for &byte in bytes {
        match byte {
            b'"' => literal.push_str("\\\""),
            b'\\' => literal.push_str("\\\\"),
            0x20..=0x7e => literal.push(char::from(byte)),
            _ => {
                use fmt::Write as _;
                write!(literal, "\\{byte:03}").unwrap();
            }
        }
    }
    literal.push('"');
    literal
}

fn diagnostic_name(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(name) => format!("{name:?}"),
        Err(_) => lua_string_literal(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segmented_and_dotted_names_have_distinct_identities_and_projections() {
        let dotted = LuaPath::global("a.b");
        let segmented = LuaPath::global("a").child("b");

        assert_ne!(dotted, segmented);
        assert_eq!(LuaAccess::for_path(&dotted).as_str(), r#"_G["a.b"]"#);
        assert_eq!(LuaAccess::for_path(&segmented).as_str(), r#"_G["a"]["b"]"#);
        assert_ne!(
            AnnotationId::for_path(&dotted),
            AnnotationId::for_path(&segmented)
        );
    }

    #[test]
    fn source_projection_preserves_arbitrary_lua_string_bytes() {
        let path = LuaPath::global(LuaName::from_bytes([0, b'"', b'\\', 0xff].as_slice()));

        assert_eq!(LuaAccess::for_path(&path).as_str(), r#"_G["\000\"\\\255"]"#);
        assert_eq!(AnnotationId::for_path(&path).as_str(), "__mlua_4x00225cff");
    }

    #[test]
    fn annotation_projection_is_unambiguous_for_empty_and_joined_segments() {
        let empty_then_a = LuaPath::global("").child("a");
        let a_then_empty = LuaPath::global("a").child("");
        let joined = LuaPath::global("a");

        assert_ne!(
            AnnotationId::for_path(&empty_then_a),
            AnnotationId::for_path(&a_then_empty)
        );
        assert_ne!(
            AnnotationId::for_path(&empty_then_a),
            AnnotationId::for_path(&joined)
        );
        assert_ne!(
            AnnotationId::for_path(&a_then_empty),
            AnnotationId::for_path(&joined)
        );
    }
}
