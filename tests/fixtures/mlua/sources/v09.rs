use mlua::{FromLua, IntoLua, Lua, MetaMethod, Result, Table, UserData, UserDataFields, UserDataMethods, Value};

pub struct Handwritten(pub i64);

impl UserData for Handwritten {
    fn add_fields<'lua, F: UserDataFields<'lua, Self>>(fields: &mut F) {
        fields.add_field_method_get("value", |_, this| Ok(this.0));
        fields.add_field_method_set("value", |_, this, value: i64| {
            this.0 = value;
            Ok(())
        });
    }

    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_method("read", |_, this, ()| Ok(this.0));
        methods.add_method_mut("add", |_, this, value: i64| {
            this.0 += value;
            Ok(this.0)
        });
        methods.add_function("new", |_, value: i64| Ok(Self(value)));
        methods.add_async_method("read_async", |_, this, ()| async move { Ok(this.0) });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
    }
}

pub struct RuntimeRegistered(pub bool);

pub fn register_runtime(lua: &Lua) -> Result<()> {
    lua.register_userdata_type::<RuntimeRegistered>(|registry| {
        registry.add_field_method_get("enabled", |_, this| Ok(this.0));
        registry.add_method("toggle", |_, this, ()| Ok(!this.0));
    })
}

pub enum WireMode {
    Fast,
    Slow,
}

impl<'lua> IntoLua<'lua> for WireMode {
    fn into_lua(self, lua: &'lua Lua) -> Result<Value<'lua>> {
        Ok(Value::String(lua.create_string(match self {
            Self::Fast => "fast",
            Self::Slow => "slow",
        })?))
    }
}

impl<'lua> FromLua<'lua> for WireMode {
    fn from_lua(value: Value<'lua>, _: &'lua Lua) -> Result<Self> {
        match value {
            Value::String(value) if value.as_bytes() == b"fast" => Ok(Self::Fast),
            Value::String(value) if value.as_bytes() == b"slow" => Ok(Self::Slow),
            _ => Err(mlua::Error::FromLuaConversionError {
                from: value.type_name(),
                to: "WireMode",
                message: None,
            }),
        }
    }
}

#[mlua::lua_module]
fn compatibility_fixture(lua: &Lua) -> Result<Table<'_>> {
    let module = lua.create_table()?;
    module.set("answer", 42_i64)?;
    Ok(module)
}

#[mlua::lua_module(name = "renamed_fixture")]
fn renamed_entrypoint(lua: &Lua) -> Result<Table<'_>> {
    let module = lua.create_table()?;
    module.set("enabled", true)?;
    Ok(module)
}
