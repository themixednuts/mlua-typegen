use crate::LuaType;

/// Maps a fully-qualified Rust type path to a Lua type.
///
/// This is used by the extraction phase to convert resolved `ty::Ty` types
/// into their Lua equivalents. The input `rust_type` should be a fully
/// qualified path string from `tcx.def_path_str()`.
pub fn map_rust_type(rust_type: &str, type_args: &[LuaType]) -> LuaType {
    match rust_type {
        // String types
        // Note: Cow<str> resolves via the generic Cow<T> unwrap + str → String path
        "std::string::String" | "alloc::string::String" | "&str" | "str" => LuaType::String,

        // Integer types
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
        | "u128" | "usize" => LuaType::Integer,

        // Float types
        "f32" | "f64" => LuaType::Number,

        // Boolean
        "bool" => LuaType::Boolean,

        // Unit / void
        "()" => LuaType::Nil,

        // Vec<T> → T[]
        "std::vec::Vec" | "alloc::vec::Vec" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            LuaType::Array(Box::new(inner))
        }

        // Option<T> → T? (flatten Option<Option<T>> → T?)
        "std::option::Option" | "core::option::Option" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            if matches!(inner, LuaType::Optional(_)) {
                inner // Already optional, don't double-wrap
            } else {
                LuaType::Optional(Box::new(inner))
            }
        }

        // HashMap<K, V> / BTreeMap<K, V> → table<K, V>
        "std::collections::HashMap" | "std::collections::hash_map::HashMap"
        | "std::collections::BTreeMap" | "std::collections::btree_map::BTreeMap" => {
            let k = type_args.first().cloned().unwrap_or(LuaType::Any);
            let v = type_args.get(1).cloned().unwrap_or(LuaType::Any);
            LuaType::Map(Box::new(k), Box::new(v))
        }

        // Result<T, _> → unwrap to T
        "std::result::Result" | "core::result::Result" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // Box<T>, Arc<T>, Rc<T> → unwrap to T
        "std::boxed::Box" | "alloc::boxed::Box"
        | "std::sync::Arc" | "alloc::sync::Arc"
        | "std::rc::Rc" | "alloc::rc::Rc" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // Mutex<T>, RwLock<T> → unwrap to T (lock wrappers transparent to Lua)
        "std::sync::Mutex" | "std::sync::RwLock"
        | "std::sync::MutexGuard" | "std::sync::RwLockReadGuard"
        | "std::sync::RwLockWriteGuard"
        | "parking_lot::Mutex" | "parking_lot::RwLock"
        | "parking_lot::MutexGuard" | "parking_lot::RwLockReadGuard"
        | "parking_lot::RwLockWriteGuard"
        | "tokio::sync::Mutex" | "tokio::sync::RwLock"
        | "tokio::sync::MutexGuard" | "tokio::sync::RwLockReadGuard"
        | "tokio::sync::RwLockWriteGuard" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // Cell/RefCell → unwrap to T
        "std::cell::Cell" | "std::cell::RefCell"
        | "std::cell::Ref" | "std::cell::RefMut" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // Path types → string
        "std::path::PathBuf" | "alloc::path::PathBuf"
        | "std::path::Path" => LuaType::String,

        // OsString/OsStr/CString/CStr → string
        "std::ffi::OsString" | "std::ffi::OsStr"
        | "std::ffi::CString" | "std::ffi::CStr"
        | "core::ffi::CStr" => LuaType::String,

        // Cow<T> generic → unwrap to T (Cow<str> already handled above as string)
        "std::borrow::Cow" | "alloc::borrow::Cow" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // Set types → T[] (Lua has no native set, array is closest)
        "std::collections::HashSet" | "std::collections::hash_set::HashSet"
        | "std::collections::BTreeSet" | "std::collections::btree_set::BTreeSet" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            LuaType::Array(Box::new(inner))
        }

        // Other sequential containers → T[]
        "std::collections::VecDeque" | "std::collections::vec_deque::VecDeque"
        | "std::collections::LinkedList" | "std::collections::linked_list::LinkedList" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            LuaType::Array(Box::new(inner))
        }

        // NonZero integer types → integer
        "std::num::NonZeroI8" | "std::num::NonZeroI16" | "std::num::NonZeroI32"
        | "std::num::NonZeroI64" | "std::num::NonZeroI128" | "std::num::NonZeroIsize"
        | "std::num::NonZeroU8" | "std::num::NonZeroU16" | "std::num::NonZeroU32"
        | "std::num::NonZeroU64" | "std::num::NonZeroU128" | "std::num::NonZeroUsize"
        | "core::num::NonZeroI8" | "core::num::NonZeroI16" | "core::num::NonZeroI32"
        | "core::num::NonZeroI64" | "core::num::NonZeroI128" | "core::num::NonZeroIsize"
        | "core::num::NonZeroU8" | "core::num::NonZeroU16" | "core::num::NonZeroU32"
        | "core::num::NonZeroU64" | "core::num::NonZeroU128" | "core::num::NonZeroUsize" => {
            LuaType::Integer
        }

        // Pin<T> → unwrap to T
        "std::pin::Pin" | "core::pin::Pin" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // MaybeUninit<T>, ManuallyDrop<T> → unwrap to T
        "std::mem::MaybeUninit" | "core::mem::MaybeUninit"
        | "std::mem::ManuallyDrop" | "core::mem::ManuallyDrop" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // OnceCell/OnceLock/LazyLock/LazyCell → unwrap to T
        "std::cell::OnceCell" | "std::sync::OnceLock"
        | "std::sync::LazyLock" | "std::cell::LazyCell" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // once_cell crate
        "once_cell::sync::OnceCell" | "once_cell::sync::Lazy"
        | "once_cell::unsync::OnceCell" | "once_cell::unsync::Lazy" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // Wrapping<T>/Saturating<T> → integer
        "std::num::Wrapping" | "core::num::Wrapping"
        | "std::num::Saturating" | "core::num::Saturating" => LuaType::Integer,

        // Atomic types → integer or boolean
        "std::sync::atomic::AtomicBool" | "core::sync::atomic::AtomicBool" => LuaType::Boolean,
        "std::sync::atomic::AtomicI8" | "std::sync::atomic::AtomicI16"
        | "std::sync::atomic::AtomicI32" | "std::sync::atomic::AtomicI64"
        | "std::sync::atomic::AtomicIsize"
        | "std::sync::atomic::AtomicU8" | "std::sync::atomic::AtomicU16"
        | "std::sync::atomic::AtomicU32" | "std::sync::atomic::AtomicU64"
        | "std::sync::atomic::AtomicUsize"
        | "core::sync::atomic::AtomicI8" | "core::sync::atomic::AtomicI16"
        | "core::sync::atomic::AtomicI32" | "core::sync::atomic::AtomicI64"
        | "core::sync::atomic::AtomicIsize"
        | "core::sync::atomic::AtomicU8" | "core::sync::atomic::AtomicU16"
        | "core::sync::atomic::AtomicU32" | "core::sync::atomic::AtomicU64"
        | "core::sync::atomic::AtomicUsize" => LuaType::Integer,

        // mlua types
        "mlua::table::Table" | "mlua::Table"
        | "mlua::prelude::LuaTable" => LuaType::Table,

        "mlua::function::Function" | "mlua::Function"
        | "mlua::prelude::LuaFunction" => LuaType::Function,

        "mlua::value::Value" | "mlua::Value"
        | "mlua::prelude::LuaValue" => LuaType::Any,

        "mlua::string::String" | "mlua::String" | "mlua::BString"
        | "mlua::prelude::LuaString" => LuaType::String,

        "mlua::thread::Thread" | "mlua::Thread"
        | "mlua::prelude::LuaThread" => LuaType::Thread,

        "mlua::types::Integer" | "mlua::Integer"
        | "mlua::prelude::LuaInteger" => LuaType::Integer,

        "mlua::types::Number" | "mlua::Number"
        | "mlua::prelude::LuaNumber" => LuaType::Number,

        "mlua::types::LightUserData" | "mlua::LightUserData"
        | "mlua::prelude::LuaLightUserData" => LuaType::Any,

        "mlua::userdata::AnyUserData" | "mlua::AnyUserData"
        | "mlua::prelude::LuaAnyUserData" => LuaType::Any,

        "mlua::multi::MultiValue" | "mlua::MultiValue"
        | "mlua::prelude::LuaMultiValue" => LuaType::Any,

        // mlua::Error → string (Lua errors are strings)
        "mlua::error::Error" | "mlua::Error"
        | "mlua::prelude::LuaError" => LuaType::String,

        // Variadic<T> → T...
        "mlua::types::Variadic" | "mlua::Variadic"
        | "mlua::prelude::LuaVariadic" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            LuaType::Variadic(Box::new(inner))
        }

        // ── Popular crate types ─────────────────────────────────────────

        // bytes crate
        "bytes::Bytes" | "bytes::BytesMut"
        | "bytes::bytes::Bytes" | "bytes::bytes_mut::BytesMut" => LuaType::String,

        // String crates (compact_str, smol_str, ecow, arcstr, flexstr, kstring)
        "compact_str::CompactString" | "smol_str::SmolStr"
        | "ecow::EcoString" | "arcstr::ArcStr"
        | "flexstr::SharedStr" | "flexstr::LocalStr"
        | "kstring::KString" | "kstring::KStringRef"
        | "bstr::BString" | "bstr::BStr" => LuaType::String,

        // Array/vec crates (arrayvec, smallvec, tinyvec, thin_vec)
        "arrayvec::ArrayVec" | "smallvec::SmallVec"
        | "tinyvec::TinyVec" | "tinyvec::ArrayVec"
        | "thin_vec::ThinVec" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            LuaType::Array(Box::new(inner))
        }

        // indexmap crate
        "indexmap::IndexMap" | "indexmap::map::IndexMap" => {
            let k = type_args.first().cloned().unwrap_or(LuaType::Any);
            let v = type_args.get(1).cloned().unwrap_or(LuaType::Any);
            LuaType::Map(Box::new(k), Box::new(v))
        }
        "indexmap::IndexSet" | "indexmap::set::IndexSet" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            LuaType::Array(Box::new(inner))
        }

        // dashmap (concurrent HashMap)
        "dashmap::DashMap" => {
            let k = type_args.first().cloned().unwrap_or(LuaType::Any);
            let v = type_args.get(1).cloned().unwrap_or(LuaType::Any);
            LuaType::Map(Box::new(k), Box::new(v))
        }
        "dashmap::DashSet" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            LuaType::Array(Box::new(inner))
        }

        // serde_json
        "serde_json::Value" | "serde_json::value::Value" => LuaType::Any,
        "serde_json::Map" | "serde_json::map::Map" => {
            let k = type_args.first().cloned().unwrap_or(LuaType::String);
            let v = type_args.get(1).cloned().unwrap_or(LuaType::Any);
            LuaType::Map(Box::new(k), Box::new(v))
        }
        "serde_json::Number" => LuaType::Number,

        // uuid, url — string-like identifiers
        "uuid::Uuid" | "url::Url" => LuaType::String,

        // chrono / time — datetime as string (ISO 8601)
        "chrono::NaiveDateTime" | "chrono::NaiveDate" | "chrono::NaiveTime"
        | "chrono::DateTime" => LuaType::String,
        "chrono::Duration" | "std::time::Duration" | "core::time::Duration" => LuaType::Number,

        // rustc-hash (FxHashMap/FxHashSet)
        "rustc_hash::FxHashMap" | "rustc_hash::map::FxHashMap"
        | "fxhash::FxHashMap" => {
            let k = type_args.first().cloned().unwrap_or(LuaType::Any);
            let v = type_args.get(1).cloned().unwrap_or(LuaType::Any);
            LuaType::Map(Box::new(k), Box::new(v))
        }
        "rustc_hash::FxHashSet" | "rustc_hash::set::FxHashSet"
        | "fxhash::FxHashSet" => {
            let inner = type_args.first().cloned().unwrap_or(LuaType::Any);
            LuaType::Array(Box::new(inner))
        }

        // arc-swap — ArcSwap<T>, ArcSwapOption<T>, Guard<T>, etc. → unwrap to T
        "arc_swap::ArcSwap" | "arc_swap::ArcSwapOption"
        | "arc_swap::ArcSwapAny"
        | "arc_swap::Guard" | "arc_swap::access::Guard" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // left-right — ReadHandle<T> and WriteHandle<T> → unwrap to T
        "left_right::ReadHandle" | "left_right::WriteHandle"
        | "left_right::ReadGuard" => {
            type_args.first().cloned().unwrap_or(LuaType::Any)
        }

        // evmap (built on left-right) — ReadHandle<K,V> / WriteHandle<K,V> → table<K, V>
        "evmap::ReadHandle" | "evmap::WriteHandle" => {
            let k = type_args.first().cloned().unwrap_or(LuaType::Any);
            let v = type_args.get(1).cloned().unwrap_or(LuaType::Any);
            LuaType::Map(Box::new(k), Box::new(v))
        }

        // anyhow/eyre error types — unwrap like Result
        "anyhow::Error" | "eyre::Report" | "eyre::EyreReport" => LuaType::Any,

        // Anything else: try heuristic matching, then fall back to class reference
        other => heuristic_type_match(other, type_args),
    }
}

/// Heuristic fallback for types not explicitly mapped.
///
/// Uses the last path segment name and type arg count to make educated guesses
/// before falling back to a Class reference. This catches many third-party crate
/// types that follow common Rust naming conventions.
fn heuristic_type_match(rust_type: &str, type_args: &[LuaType]) -> LuaType {
    let last_segment = rust_type.rsplit("::").next().unwrap_or(rust_type);

    // Types ending in "String" or "Str" with no type args → string
    if type_args.is_empty()
        && (last_segment.ends_with("String") || last_segment.ends_with("Str"))
        && last_segment != "String" // already handled
    {
        return LuaType::String;
    }

    // Types ending in "Vec" with 1 type arg → array
    if last_segment.ends_with("Vec") && type_args.len() == 1 {
        return LuaType::Array(Box::new(type_args[0].clone()));
    }

    // Types ending in "Map" with 2 type args → map
    if last_segment.ends_with("Map") && type_args.len() >= 2 {
        return LuaType::Map(
            Box::new(type_args[0].clone()),
            Box::new(type_args[1].clone()),
        );
    }

    // Types ending in "Set" with 1 type arg → array
    if last_segment.ends_with("Set") && type_args.len() == 1 {
        return LuaType::Array(Box::new(type_args[0].clone()));
    }

    // Types ending in "Mutex" / "RwLock" / "Lock" / "Guard" with 1 type arg → unwrap
    if type_args.len() == 1
        && (last_segment.ends_with("Mutex")
            || last_segment.ends_with("RwLock")
            || last_segment.ends_with("Lock")
            || last_segment.ends_with("Guard"))
    {
        return type_args[0].clone();
    }

    // Default: use last segment as class name
    LuaType::Class(last_segment.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Primitive types ────────────────────────────────────────────────

    #[test]
    fn bool_maps_to_boolean() {
        assert_eq!(map_rust_type("bool", &[]), LuaType::Boolean);
    }

    #[test]
    fn all_integer_types() {
        for ty in ["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Integer, "failed for {ty}");
        }
    }

    #[test]
    fn all_float_types() {
        assert_eq!(map_rust_type("f32", &[]), LuaType::Number);
        assert_eq!(map_rust_type("f64", &[]), LuaType::Number);
    }

    #[test]
    fn unit_maps_to_nil() {
        assert_eq!(map_rust_type("()", &[]), LuaType::Nil);
    }

    // ── String types ───────────────────────────────────────────────────

    #[test]
    fn all_string_types() {
        for ty in [
            "std::string::String",
            "alloc::string::String",
            "&str",
            "str",
        ] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::String, "failed for {ty}");
        }
    }

    // ── Generic containers ─────────────────────────────────────────────

    #[test]
    fn vec_maps_to_array() {
        assert_eq!(
            map_rust_type("std::vec::Vec", &[LuaType::Integer]),
            LuaType::Array(Box::new(LuaType::Integer))
        );
        assert_eq!(
            map_rust_type("alloc::vec::Vec", &[LuaType::String]),
            LuaType::Array(Box::new(LuaType::String))
        );
    }

    #[test]
    fn vec_no_args_defaults_to_any() {
        assert_eq!(
            map_rust_type("std::vec::Vec", &[]),
            LuaType::Array(Box::new(LuaType::Any))
        );
    }

    #[test]
    fn option_maps_to_optional() {
        assert_eq!(
            map_rust_type("std::option::Option", &[LuaType::String]),
            LuaType::Optional(Box::new(LuaType::String))
        );
        assert_eq!(
            map_rust_type("core::option::Option", &[LuaType::Boolean]),
            LuaType::Optional(Box::new(LuaType::Boolean))
        );
    }

    #[test]
    fn hashmap_maps_to_map() {
        assert_eq!(
            map_rust_type("std::collections::HashMap", &[LuaType::String, LuaType::Integer]),
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Integer))
        );
    }

    #[test]
    fn btreemap_maps_to_map() {
        assert_eq!(
            map_rust_type("std::collections::BTreeMap", &[LuaType::String, LuaType::Boolean]),
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Boolean))
        );
    }

    #[test]
    fn hashmap_full_path() {
        assert_eq!(
            map_rust_type("std::collections::hash_map::HashMap", &[LuaType::String, LuaType::Any]),
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Any))
        );
    }

    #[test]
    fn btreemap_full_path() {
        assert_eq!(
            map_rust_type("std::collections::btree_map::BTreeMap", &[LuaType::Integer, LuaType::String]),
            LuaType::Map(Box::new(LuaType::Integer), Box::new(LuaType::String))
        );
    }

    #[test]
    fn map_no_args_defaults_to_any() {
        assert_eq!(
            map_rust_type("std::collections::HashMap", &[]),
            LuaType::Map(Box::new(LuaType::Any), Box::new(LuaType::Any))
        );
    }

    // ── Result unwrapping ──────────────────────────────────────────────

    #[test]
    fn result_unwraps_to_ok_type() {
        assert_eq!(
            map_rust_type("std::result::Result", &[LuaType::String, LuaType::Any]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("core::result::Result", &[LuaType::Boolean, LuaType::Any]),
            LuaType::Boolean
        );
    }

    #[test]
    fn result_no_args_defaults_to_any() {
        assert_eq!(map_rust_type("std::result::Result", &[]), LuaType::Any);
    }

    // ── mlua types ─────────────────────────────────────────────────────

    #[test]
    fn mlua_table_types() {
        for ty in ["mlua::table::Table", "mlua::Table", "mlua::prelude::LuaTable"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Table, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_function_types() {
        for ty in ["mlua::function::Function", "mlua::Function", "mlua::prelude::LuaFunction"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Function, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_value_types() {
        for ty in ["mlua::value::Value", "mlua::Value", "mlua::prelude::LuaValue"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Any, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_string_types() {
        for ty in ["mlua::string::String", "mlua::String", "mlua::BString", "mlua::prelude::LuaString"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::String, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_thread_types() {
        for ty in ["mlua::thread::Thread", "mlua::Thread", "mlua::prelude::LuaThread"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Thread, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_integer_types() {
        for ty in ["mlua::types::Integer", "mlua::Integer", "mlua::prelude::LuaInteger"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Integer, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_number_types() {
        for ty in ["mlua::types::Number", "mlua::Number", "mlua::prelude::LuaNumber"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Number, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_lightuserdata_types() {
        for ty in ["mlua::types::LightUserData", "mlua::LightUserData", "mlua::prelude::LuaLightUserData"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Any, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_anyuserdata_types() {
        for ty in ["mlua::userdata::AnyUserData", "mlua::AnyUserData", "mlua::prelude::LuaAnyUserData"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Any, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_multivalue_types() {
        for ty in ["mlua::multi::MultiValue", "mlua::MultiValue", "mlua::prelude::LuaMultiValue"] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Any, "failed for {ty}");
        }
    }

    #[test]
    fn mlua_variadic_types() {
        for ty in ["mlua::types::Variadic", "mlua::Variadic", "mlua::prelude::LuaVariadic"] {
            assert_eq!(
                map_rust_type(ty, &[LuaType::String]),
                LuaType::Variadic(Box::new(LuaType::String)),
                "failed for {ty}"
            );
        }
    }

    #[test]
    fn mlua_variadic_no_args_defaults_to_any() {
        assert_eq!(
            map_rust_type("mlua::Variadic", &[]),
            LuaType::Variadic(Box::new(LuaType::Any))
        );
    }

    // ── Smart pointer unwrapping ──────────────────────────────────────

    #[test]
    fn box_unwraps_to_inner() {
        assert_eq!(
            map_rust_type("std::boxed::Box", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("alloc::boxed::Box", &[LuaType::Integer]),
            LuaType::Integer
        );
    }

    #[test]
    fn arc_unwraps_to_inner() {
        assert_eq!(
            map_rust_type("std::sync::Arc", &[LuaType::Boolean]),
            LuaType::Boolean
        );
        assert_eq!(
            map_rust_type("alloc::sync::Arc", &[LuaType::Class("Player".to_string())]),
            LuaType::Class("Player".to_string())
        );
    }

    #[test]
    fn rc_unwraps_to_inner() {
        assert_eq!(
            map_rust_type("std::rc::Rc", &[LuaType::Number]),
            LuaType::Number
        );
        assert_eq!(
            map_rust_type("alloc::rc::Rc", &[LuaType::Table]),
            LuaType::Table
        );
    }

    #[test]
    fn smart_pointer_no_args_defaults_to_any() {
        assert_eq!(map_rust_type("std::boxed::Box", &[]), LuaType::Any);
        assert_eq!(map_rust_type("std::sync::Arc", &[]), LuaType::Any);
        assert_eq!(map_rust_type("std::rc::Rc", &[]), LuaType::Any);
    }

    #[test]
    fn nested_box_in_vec() {
        // Vec<Box<String>> → string[] (Box already unwrapped by caller)
        assert_eq!(
            map_rust_type("std::vec::Vec", &[LuaType::String]),
            LuaType::Array(Box::new(LuaType::String))
        );
    }

    // ── Lock wrapper unwrapping ─────────────────────────────────────────

    #[test]
    fn mutex_unwraps_to_inner() {
        assert_eq!(
            map_rust_type("std::sync::Mutex", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("std::sync::RwLock", &[LuaType::Integer]),
            LuaType::Integer
        );
    }

    #[test]
    fn parking_lot_locks_unwrap() {
        assert_eq!(
            map_rust_type("parking_lot::Mutex", &[LuaType::Boolean]),
            LuaType::Boolean
        );
        assert_eq!(
            map_rust_type("parking_lot::RwLock", &[LuaType::Number]),
            LuaType::Number
        );
    }

    #[test]
    fn tokio_locks_unwrap() {
        assert_eq!(
            map_rust_type("tokio::sync::Mutex", &[LuaType::Table]),
            LuaType::Table
        );
        assert_eq!(
            map_rust_type("tokio::sync::RwLock", &[LuaType::Function]),
            LuaType::Function
        );
    }

    #[test]
    fn lock_guards_unwrap() {
        assert_eq!(
            map_rust_type("std::sync::MutexGuard", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("std::sync::RwLockReadGuard", &[LuaType::Integer]),
            LuaType::Integer
        );
    }

    #[test]
    fn cell_types_unwrap() {
        assert_eq!(
            map_rust_type("std::cell::Cell", &[LuaType::Boolean]),
            LuaType::Boolean
        );
        assert_eq!(
            map_rust_type("std::cell::RefCell", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("std::cell::Ref", &[LuaType::Number]),
            LuaType::Number
        );
        assert_eq!(
            map_rust_type("std::cell::RefMut", &[LuaType::Integer]),
            LuaType::Integer
        );
    }

    // ── Path/OS types ───────────────────────────────────────────────────

    #[test]
    fn path_types_map_to_string() {
        assert_eq!(map_rust_type("std::path::PathBuf", &[]), LuaType::String);
        assert_eq!(map_rust_type("std::path::Path", &[]), LuaType::String);
    }

    #[test]
    fn os_string_types_map_to_string() {
        assert_eq!(map_rust_type("std::ffi::OsString", &[]), LuaType::String);
        assert_eq!(map_rust_type("std::ffi::OsStr", &[]), LuaType::String);
    }

    // ── Generic Cow ─────────────────────────────────────────────────────

    #[test]
    fn cow_generic_unwraps() {
        // Cow<str> is already handled as string above; generic Cow<T> unwraps
        assert_eq!(
            map_rust_type("std::borrow::Cow", &[LuaType::Integer]),
            LuaType::Integer
        );
    }

    // ── Set types ───────────────────────────────────────────────────────

    #[test]
    fn hashset_maps_to_array() {
        assert_eq!(
            map_rust_type("std::collections::HashSet", &[LuaType::String]),
            LuaType::Array(Box::new(LuaType::String))
        );
        assert_eq!(
            map_rust_type("std::collections::hash_set::HashSet", &[LuaType::Integer]),
            LuaType::Array(Box::new(LuaType::Integer))
        );
    }

    #[test]
    fn btreeset_maps_to_array() {
        assert_eq!(
            map_rust_type("std::collections::BTreeSet", &[LuaType::String]),
            LuaType::Array(Box::new(LuaType::String))
        );
        assert_eq!(
            map_rust_type("std::collections::btree_set::BTreeSet", &[LuaType::Number]),
            LuaType::Array(Box::new(LuaType::Number))
        );
    }

    // ── Sequential containers ───────────────────────────────────────────

    #[test]
    fn vecdeque_maps_to_array() {
        assert_eq!(
            map_rust_type("std::collections::VecDeque", &[LuaType::String]),
            LuaType::Array(Box::new(LuaType::String))
        );
        assert_eq!(
            map_rust_type("std::collections::vec_deque::VecDeque", &[LuaType::Integer]),
            LuaType::Array(Box::new(LuaType::Integer))
        );
    }

    #[test]
    fn linkedlist_maps_to_array() {
        assert_eq!(
            map_rust_type("std::collections::LinkedList", &[LuaType::Boolean]),
            LuaType::Array(Box::new(LuaType::Boolean))
        );
    }

    // ── NonZero types ───────────────────────────────────────────────────

    #[test]
    fn nonzero_types_map_to_integer() {
        for ty in [
            "std::num::NonZeroU8", "std::num::NonZeroU16", "std::num::NonZeroU32",
            "std::num::NonZeroU64", "std::num::NonZeroU128", "std::num::NonZeroUsize",
            "std::num::NonZeroI8", "std::num::NonZeroI16", "std::num::NonZeroI32",
            "std::num::NonZeroI64", "std::num::NonZeroI128", "std::num::NonZeroIsize",
            "core::num::NonZeroU32", "core::num::NonZeroI64",
        ] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Integer, "failed for {ty}");
        }
    }

    // ── Pin ─────────────────────────────────────────────────────────────

    #[test]
    fn pin_unwraps_to_inner() {
        assert_eq!(
            map_rust_type("std::pin::Pin", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("core::pin::Pin", &[LuaType::Function]),
            LuaType::Function
        );
    }

    // ── Double-unwrap / compound nesting ────────────────────────────────

    #[test]
    fn arc_mutex_unwraps_both() {
        // Arc<Mutex<String>> → Arc unwraps to Mutex<String>, Mutex unwraps to String
        // At typemap level: Arc gets inner = String (already resolved by caller)
        assert_eq!(
            map_rust_type("std::sync::Arc", &[LuaType::String]),
            LuaType::String
        );
    }

    #[test]
    fn result_of_option() {
        // Result<Option<String>> → unwraps Result → Optional(String)
        assert_eq!(
            map_rust_type("std::result::Result", &[
                LuaType::Optional(Box::new(LuaType::String)),
                LuaType::Any,
            ]),
            LuaType::Optional(Box::new(LuaType::String))
        );
    }

    #[test]
    fn option_of_result() {
        // Option<Result<String>> → Result already unwrapped by caller → Optional(String)
        assert_eq!(
            map_rust_type("std::option::Option", &[LuaType::String]),
            LuaType::Optional(Box::new(LuaType::String))
        );
    }

    #[test]
    fn box_of_vec() {
        // Box<Vec<i32>> → Box unwraps → Array(Integer)
        assert_eq!(
            map_rust_type("std::boxed::Box", &[LuaType::Array(Box::new(LuaType::Integer))]),
            LuaType::Array(Box::new(LuaType::Integer))
        );
    }

    // ── UserData class fallback ────────────────────────────────────────

    #[test]
    fn unknown_type_maps_to_class() {
        assert_eq!(
            map_rust_type("my_crate::Buffer", &[]),
            LuaType::Class("Buffer".to_string())
        );
    }

    #[test]
    fn deeply_nested_path_uses_last_segment() {
        assert_eq!(
            map_rust_type("a::b::c::d::MyType", &[]),
            LuaType::Class("MyType".to_string())
        );
    }

    #[test]
    fn bare_name_maps_to_class() {
        assert_eq!(
            map_rust_type("SomeStruct", &[]),
            LuaType::Class("SomeStruct".to_string())
        );
    }

    // ── Nested generic types ───────────────────────────────────────────

    #[test]
    fn vec_of_optional() {
        assert_eq!(
            map_rust_type("std::vec::Vec", &[LuaType::Optional(Box::new(LuaType::String))]),
            LuaType::Array(Box::new(LuaType::Optional(Box::new(LuaType::String))))
        );
    }

    #[test]
    fn option_of_vec() {
        assert_eq!(
            map_rust_type("std::option::Option", &[LuaType::Array(Box::new(LuaType::Integer))]),
            LuaType::Optional(Box::new(LuaType::Array(Box::new(LuaType::Integer))))
        );
    }

    #[test]
    fn result_of_vec() {
        assert_eq!(
            map_rust_type("std::result::Result", &[LuaType::Array(Box::new(LuaType::String)), LuaType::Any]),
            LuaType::Array(Box::new(LuaType::String))
        );
    }

    #[test]
    fn map_with_class_values() {
        assert_eq!(
            map_rust_type("std::collections::HashMap", &[
                LuaType::String,
                LuaType::Class("Player".to_string()),
            ]),
            LuaType::Map(
                Box::new(LuaType::String),
                Box::new(LuaType::Class("Player".to_string()))
            )
        );
    }

    // ── Popular crate types ────────────────────────────────────────────

    #[test]
    fn bytes_crate_types() {
        assert_eq!(map_rust_type("bytes::Bytes", &[]), LuaType::String);
        assert_eq!(map_rust_type("bytes::BytesMut", &[]), LuaType::String);
    }

    #[test]
    fn string_crate_types() {
        for ty in [
            "compact_str::CompactString",
            "smol_str::SmolStr",
            "ecow::EcoString",
            "arcstr::ArcStr",
            "kstring::KString",
            "bstr::BString",
            "bstr::BStr",
        ] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::String, "failed for {ty}");
        }
    }

    #[test]
    fn arrayvec_crate_types() {
        for ty in ["arrayvec::ArrayVec", "smallvec::SmallVec", "tinyvec::TinyVec", "thin_vec::ThinVec"] {
            assert_eq!(
                map_rust_type(ty, &[LuaType::Integer]),
                LuaType::Array(Box::new(LuaType::Integer)),
                "failed for {ty}"
            );
        }
    }

    #[test]
    fn indexmap_types() {
        assert_eq!(
            map_rust_type("indexmap::IndexMap", &[LuaType::String, LuaType::Integer]),
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Integer))
        );
        assert_eq!(
            map_rust_type("indexmap::IndexSet", &[LuaType::String]),
            LuaType::Array(Box::new(LuaType::String))
        );
    }

    #[test]
    fn dashmap_types() {
        assert_eq!(
            map_rust_type("dashmap::DashMap", &[LuaType::String, LuaType::Any]),
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Any))
        );
        assert_eq!(
            map_rust_type("dashmap::DashSet", &[LuaType::Integer]),
            LuaType::Array(Box::new(LuaType::Integer))
        );
    }

    #[test]
    fn serde_json_types() {
        assert_eq!(map_rust_type("serde_json::Value", &[]), LuaType::Any);
        assert_eq!(map_rust_type("serde_json::Number", &[]), LuaType::Number);
        assert_eq!(
            map_rust_type("serde_json::Map", &[LuaType::String, LuaType::Any]),
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Any))
        );
    }

    #[test]
    fn uuid_url_types() {
        assert_eq!(map_rust_type("uuid::Uuid", &[]), LuaType::String);
        assert_eq!(map_rust_type("url::Url", &[]), LuaType::String);
    }

    #[test]
    fn chrono_time_types() {
        assert_eq!(map_rust_type("chrono::DateTime", &[]), LuaType::String);
        assert_eq!(map_rust_type("chrono::NaiveDate", &[]), LuaType::String);
        assert_eq!(map_rust_type("chrono::Duration", &[]), LuaType::Number);
        assert_eq!(map_rust_type("std::time::Duration", &[]), LuaType::Number);
        assert_eq!(map_rust_type("core::time::Duration", &[]), LuaType::Number);
    }

    #[test]
    fn anyhow_eyre_types() {
        assert_eq!(map_rust_type("anyhow::Error", &[]), LuaType::Any);
        assert_eq!(map_rust_type("eyre::Report", &[]), LuaType::Any);
    }

    // ── Heuristic fallback tests ───────────────────────────────────────

    #[test]
    fn heuristic_string_suffix() {
        // Unknown types ending in "String" or "Str" → string
        assert_eq!(map_rust_type("my_crate::MyCustomString", &[]), LuaType::String);
        assert_eq!(map_rust_type("foo::InternStr", &[]), LuaType::String);
    }

    #[test]
    fn heuristic_vec_suffix() {
        // Unknown types ending in "Vec" with 1 type arg → array
        assert_eq!(
            map_rust_type("custom::MyVec", &[LuaType::Integer]),
            LuaType::Array(Box::new(LuaType::Integer))
        );
    }

    #[test]
    fn heuristic_map_suffix() {
        // Unknown types ending in "Map" with 2 type args → map
        assert_eq!(
            map_rust_type("custom::MyMap", &[LuaType::String, LuaType::Boolean]),
            LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Boolean))
        );
    }

    #[test]
    fn heuristic_set_suffix() {
        // Unknown types ending in "Set" with 1 type arg → array
        assert_eq!(
            map_rust_type("custom::MySet", &[LuaType::String]),
            LuaType::Array(Box::new(LuaType::String))
        );
    }

    #[test]
    fn heuristic_lock_suffix() {
        // Unknown types ending in "Mutex"/"Lock" with 1 type arg → unwrap
        assert_eq!(
            map_rust_type("custom::MyMutex", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("custom::FancyLock", &[LuaType::Integer]),
            LuaType::Integer
        );
    }

    #[test]
    fn heuristic_does_not_match_without_type_args() {
        // Vec suffix but no type args → class (not array)
        assert_eq!(
            map_rust_type("custom::MyVec", &[]),
            LuaType::Class("MyVec".to_string())
        );
    }

    #[test]
    fn heuristic_does_not_match_wrong_arg_count() {
        // Map suffix with 1 arg → class (needs 2)
        assert_eq!(
            map_rust_type("custom::MyMap", &[LuaType::String]),
            LuaType::Class("MyMap".to_string())
        );
    }

    // ── CString/CStr types ──────────────────────────────────────────────

    #[test]
    fn cstring_types_map_to_string() {
        assert_eq!(map_rust_type("std::ffi::CString", &[]), LuaType::String);
        assert_eq!(map_rust_type("std::ffi::CStr", &[]), LuaType::String);
        assert_eq!(map_rust_type("core::ffi::CStr", &[]), LuaType::String);
    }

    // ── MaybeUninit / ManuallyDrop ───────────────────────────────────────

    #[test]
    fn maybe_uninit_unwraps() {
        assert_eq!(
            map_rust_type("std::mem::MaybeUninit", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("core::mem::MaybeUninit", &[LuaType::Integer]),
            LuaType::Integer
        );
    }

    #[test]
    fn manually_drop_unwraps() {
        assert_eq!(
            map_rust_type("std::mem::ManuallyDrop", &[LuaType::Boolean]),
            LuaType::Boolean
        );
        assert_eq!(
            map_rust_type("core::mem::ManuallyDrop", &[LuaType::Number]),
            LuaType::Number
        );
    }

    // ── OnceCell / OnceLock / Lazy types ─────────────────────────────────

    #[test]
    fn oncecell_types_unwrap() {
        assert_eq!(
            map_rust_type("std::cell::OnceCell", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("std::sync::OnceLock", &[LuaType::Integer]),
            LuaType::Integer
        );
        assert_eq!(
            map_rust_type("std::sync::LazyLock", &[LuaType::Boolean]),
            LuaType::Boolean
        );
        assert_eq!(
            map_rust_type("std::cell::LazyCell", &[LuaType::Number]),
            LuaType::Number
        );
    }

    #[test]
    fn once_cell_crate_types_unwrap() {
        assert_eq!(
            map_rust_type("once_cell::sync::OnceCell", &[LuaType::String]),
            LuaType::String
        );
        assert_eq!(
            map_rust_type("once_cell::sync::Lazy", &[LuaType::Integer]),
            LuaType::Integer
        );
        assert_eq!(
            map_rust_type("once_cell::unsync::OnceCell", &[LuaType::Boolean]),
            LuaType::Boolean
        );
        assert_eq!(
            map_rust_type("once_cell::unsync::Lazy", &[LuaType::Number]),
            LuaType::Number
        );
    }

    // ── Wrapping / Saturating ────────────────────────────────────────────

    #[test]
    fn wrapping_saturating_map_to_integer() {
        assert_eq!(map_rust_type("std::num::Wrapping", &[LuaType::Integer]), LuaType::Integer);
        assert_eq!(map_rust_type("core::num::Wrapping", &[LuaType::Integer]), LuaType::Integer);
        assert_eq!(map_rust_type("std::num::Saturating", &[LuaType::Integer]), LuaType::Integer);
        assert_eq!(map_rust_type("core::num::Saturating", &[LuaType::Integer]), LuaType::Integer);
    }

    // ── Atomic types ─────────────────────────────────────────────────────

    #[test]
    fn atomic_bool_maps_to_boolean() {
        assert_eq!(map_rust_type("std::sync::atomic::AtomicBool", &[]), LuaType::Boolean);
        assert_eq!(map_rust_type("core::sync::atomic::AtomicBool", &[]), LuaType::Boolean);
    }

    #[test]
    fn atomic_integer_types() {
        for ty in [
            "std::sync::atomic::AtomicI32",
            "std::sync::atomic::AtomicU64",
            "std::sync::atomic::AtomicUsize",
            "core::sync::atomic::AtomicI64",
        ] {
            assert_eq!(map_rust_type(ty, &[]), LuaType::Integer, "failed for {ty}");
        }
    }

    // ── Double optional flattening ───────────────────────────────────────

    #[test]
    fn double_optional_flattens() {
        // Option<Option<String>> → string? (not string??)
        assert_eq!(
            map_rust_type("std::option::Option", &[LuaType::Optional(Box::new(LuaType::String))]),
            LuaType::Optional(Box::new(LuaType::String))
        );
    }

    // ── Fn trait types in typemap (heuristic) ──────────────────────────

    #[test]
    fn heuristic_fn_traits_dont_interfere() {
        // Fn-family traits are handled in extract.rs via dyn Trait / FnPtr,
        // not typemap. A bare "Fn" in typemap would just be a class.
        assert_eq!(
            map_rust_type("my_crate::Callback", &[]),
            LuaType::Class("Callback".to_string())
        );
    }

    #[test]
    fn heuristic_regular_class_unaffected() {
        // Normal UserData types still become classes
        assert_eq!(
            map_rust_type("my_crate::Player", &[]),
            LuaType::Class("Player".to_string())
        );
        assert_eq!(
            map_rust_type("game::Entity", &[]),
            LuaType::Class("Entity".to_string())
        );
    }

    // ── FxHashMap / FxHashSet ───────────────────────────────────────────

    #[test]
    fn fxhashmap_maps_to_map() {
        for ty in ["rustc_hash::FxHashMap", "fxhash::FxHashMap"] {
            assert_eq!(
                map_rust_type(ty, &[LuaType::String, LuaType::Integer]),
                LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Integer)),
                "failed for {ty}"
            );
        }
    }

    #[test]
    fn fxhashset_maps_to_array() {
        for ty in ["rustc_hash::FxHashSet", "fxhash::FxHashSet"] {
            assert_eq!(
                map_rust_type(ty, &[LuaType::String]),
                LuaType::Array(Box::new(LuaType::String)),
                "failed for {ty}"
            );
        }
    }

    // ── arc-swap ────────────────────────────────────────────────────────

    #[test]
    fn arc_swap_unwraps() {
        for ty in ["arc_swap::ArcSwap", "arc_swap::ArcSwapOption", "arc_swap::Guard"] {
            assert_eq!(
                map_rust_type(ty, &[LuaType::String]),
                LuaType::String,
                "failed for {ty}"
            );
        }
    }

    // ── left-right ──────────────────────────────────────────────────────

    #[test]
    fn left_right_unwraps() {
        for ty in ["left_right::ReadHandle", "left_right::WriteHandle", "left_right::ReadGuard"] {
            assert_eq!(
                map_rust_type(ty, &[LuaType::Table]),
                LuaType::Table,
                "failed for {ty}"
            );
        }
    }

    #[test]
    fn evmap_maps_to_map() {
        for ty in ["evmap::ReadHandle", "evmap::WriteHandle"] {
            assert_eq!(
                map_rust_type(ty, &[LuaType::String, LuaType::Integer]),
                LuaType::Map(Box::new(LuaType::String), Box::new(LuaType::Integer)),
                "failed for {ty}"
            );
        }
    }
}
