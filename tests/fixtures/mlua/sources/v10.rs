use mlua::{FromLua, IntoLua, Lua, MetaMethod, Result, Table, UserData, UserDataFields, UserDataMethods, UserDataRegistry, Value};

pub struct Handwritten(pub i64);

impl UserData for Handwritten {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("value", |_, this| Ok(this.0));
        fields.add_field_method_set("value", |_, this, value: i64| {
            this.0 = value;
            Ok(())
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("read", |_, this, ()| Ok(this.0));
        methods.add_method_mut("add", |_, this, value: i64| {
            this.0 += value;
            Ok(this.0)
        });
        methods.add_function("new", |_, value: i64| Ok(Self(value)));
        methods.add_async_method("read_async", |_, this, ()| async move { Ok(this.0) });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.0.to_string()));
    }

    fn register(registry: &mut UserDataRegistry<Self>) {
        Self::add_fields(registry);
        Self::add_methods(registry);
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

impl IntoLua for WireMode {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(match self {
            Self::Fast => "fast",
            Self::Slow => "slow",
        })?))
    }
}

impl FromLua for WireMode {
    fn from_lua(value: Value, _: &Lua) -> Result<Self> {
        match value {
            Value::String(value) if value.as_bytes() == b"fast" => Ok(Self::Fast),
            Value::String(value) if value.as_bytes() == b"slow" => Ok(Self::Slow),
            _ => Err(mlua::Error::FromLuaConversionError {
                from: value.type_name(),
                to: "WireMode".to_owned(),
                message: None,
            }),
        }
    }
}

#[mlua::lua_module]
fn compatibility_fixture(lua: &Lua) -> Result<Table> {
    let module = lua.create_table()?;
    module.set("answer", 42_i64)?;
    Ok(module)
}

#[mlua::lua_module(name = "renamed_fixture")]
fn renamed_entrypoint(lua: &Lua) -> Result<Table> {
    let module = lua.create_table()?;
    module.set("enabled", true)?;
    Ok(module)
}
